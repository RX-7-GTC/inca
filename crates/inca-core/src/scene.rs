//! Scene graph: DAG nodes (transform + shape), construction history, attributes, selection,
//! undo snapshots and the `.inca` JSON format (the same layout the JS app writes).

use crate::anim::{Anim, AnimCurve, AnimLayer, MergeLayerMode, MergeLayerOptions};
use crate::curves::Curve;
use crate::materials::{Material, Place2D, ShadingData, Texture};
use crate::math::*;
use crate::mesh::{OpOut, PolyMesh};
use crate::ops::{self, OpCtx};
use crate::params::Params;
use crate::primitives;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

pub type Id = String;

pub const CHANNELS: [&str; 10] = ["translateX", "translateY", "translateZ", "rotateX", "rotateY", "rotateZ", "scaleX", "scaleY", "scaleZ", "visibility"];

pub fn alias(attr: &str) -> &str {
    match attr {
        "tx" => "translateX",
        "ty" => "translateY",
        "tz" => "translateZ",
        "rx" => "rotateX",
        "ry" => "rotateY",
        "rz" => "rotateZ",
        "sx" => "scaleX",
        "sy" => "scaleY",
        "sz" => "scaleZ",
        "v" => "visibility",
        "t" => "translate",
        "r" => "rotate",
        "s" => "scale",
        "ro" => "rotateOrder",
        a => a,
    }
}

// ------------------------------------------------------------------ data

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Xform {
    pub t: [f64; 3],
    pub r: [f64; 3],
    pub s: [f64; 3],
    pub ro: String,
    /// pivot (rotate/scale pivot, local)
    pub p: [f64; 3],
    /// pivot translate (keeps the object in place when the pivot moves)
    pub pt: [f64; 3],
    /// joint orient (joints only): local = T * JO * R * S
    #[serde(default)]
    pub jo: [f64; 3],
    /// Shear components: shearXY, shearXZ, shearYZ.
    #[serde(default)]
    pub sh: [f64; 3],
    /// Rotate axis, in degrees.
    #[serde(default)]
    pub ra: [f64; 3],
    /// Whether this node inherits its parent's transform.
    #[serde(default = "default_true")]
    pub it: bool,
    /// Transform offset parent matrix (column-major, applied between parent and local).
    #[serde(default = "identity4")]
    pub opm: [[f64; 4]; 4],
    #[serde(default)]
    pub lim: Limits,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    #[serde(default)]
    pub on: bool,
    #[serde(default = "limit_min_default")]
    pub min: [f64; 3],
    #[serde(default = "limit_max_default")]
    pub max: [f64; 3],
}

fn limit_min_default() -> [f64; 3] {
    [-1.0e6; 3]
}

fn limit_max_default() -> [f64; 3] {
    [1.0e6; 3]
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub translate: Limit,
    #[serde(default)]
    pub rotate: Limit,
    #[serde(default)]
    pub scale: Limit,
}

impl Default for Limit {
    fn default() -> Self {
        Limit { on: false, min: limit_min_default(), max: limit_max_default() }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            translate: Limit::default(),
            rotate: Limit { on: false, min: [-360.0; 3], max: [360.0; 3] },
            scale: Limit::default(),
        }
    }
}

fn clamp_limit(x: f64, l: &Limit, i: usize) -> f64 {
    if l.on {
        x.clamp(l.min[i], l.max[i])
    } else {
        x
    }
}

/// Parse Maya limit attribute names: limitXMin, limitRXMax, limitSXEnable, etc.
/// Returns (group 0=translate/1=rotate/2=scale, axis, kind 0=min/1=max/2=enable).
fn limit_attr(a: &str) -> Option<(u8, usize, u8)> {
    let (group, rest) = if let Some(r) = a.strip_prefix("limitR") {
        (1, r)
    } else if let Some(r) = a.strip_prefix("limitS") {
        (2, r)
    } else if let Some(r) = a.strip_prefix("limit") {
        (0, r)
    } else {
        return None;
    };
    let axis = match rest.chars().next()? {
        'X' => 0,
        'Y' => 1,
        'Z' => 2,
        _ => return None,
    };
    let kind = if rest.ends_with("Min") {
        0
    } else if rest.ends_with("Max") {
        1
    } else if rest.ends_with("Enable") {
        2
    } else {
        return None;
    };
    Some((group, axis, kind))
}

fn default_true() -> bool {
    true
}

fn identity4() -> [[f64; 4]; 4] {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
}

impl Default for Xform {
    fn default() -> Self {
        Xform { t: [0.0; 3], r: [0.0; 3], s: [1.0; 3], ro: "xyz".into(), p: [0.0; 3], pt: [0.0; 3], jo: [0.0; 3], sh: [0.0; 3], ra: [0.0; 3], it: true, opm: identity4(), lim: Limits::default() }
    }
}

impl Xform {
    pub fn jo_quat(&self) -> DQuat {
        quat_from_euler_deg(self.jo, "xyz")
    }

    pub fn opm_matrix(&self) -> DMat4 {
        DMat4::from_cols_array_2d(&self.opm)
    }

    fn shear_matrix(&self) -> DMat4 {
        DMat4::from_cols(
            DVec4::new(1.0, 0.0, 0.0, 0.0),
            DVec4::new(self.sh[0], 1.0, 0.0, 0.0),
            DVec4::new(self.sh[1], self.sh[2], 1.0, 0.0),
            DVec4::W,
        )
    }

    /// Rotation/shear/scale part of the local matrix (no translation or pivot).
    pub fn basis(&self) -> DMat4 {
        let ra = quat_from_euler_deg(self.ra, "xyz");
        let r = ra * quat_from_euler_deg(self.r, &self.ro) * ra.inverse();
        DMat4::from_quat(self.jo_quat() * r) * self.shear_matrix() * DMat4::from_scale(v3(self.s))
    }

    /// local = T(t + pt + p) * (R * SH * S) * T(-p)
    pub fn matrix(&self) -> DMat4 {
        let basis = self.basis();
        let p = v3(self.p);
        let rp = basis.transform_point3(p);
        let pos = v3(self.t) + v3(self.pt) + p - rp;
        DMat4::from_translation(pos) * basis
    }
}

/// Split a desired basis matrix into scale, shear and rotation, removing joint orient and
/// rotate axis. Returns None for a degenerate matrix.
fn decompose_basis(basis: DMat3, jo: DQuat, ra: DQuat) -> Option<(DVec3, [f64; 3], DQuat)> {
    let b1 = basis.x_axis;
    let mut sx = b1.length();
    if sx < 1e-12 || !sx.is_finite() {
        return None;
    }
    if basis.determinant() < 0.0 {
        sx = -sx;
    }
    let r1 = b1 / sx;
    let d2 = basis.y_axis - r1 * basis.y_axis.dot(r1);
    let sy = d2.length();
    if sy < 1e-12 || !sy.is_finite() {
        return None;
    }
    let r2 = d2 / sy;
    let shxy = basis.y_axis.dot(r1) / sy;
    let d3 = basis.z_axis - r1 * basis.z_axis.dot(r1) - r2 * basis.z_axis.dot(r2);
    let sz = d3.length();
    if sz < 1e-12 || !sz.is_finite() {
        return None;
    }
    let r3 = d3 / sz;
    let shxz = basis.z_axis.dot(r1) / sz;
    let shyz = basis.z_axis.dot(r2) / sz;
    let reff = DMat3::from_cols(r1, r2, r3);
    let q_eff = DQuat::from_mat3(&reff).normalize();
    let q = ra.inverse() * jo.inverse() * q_eff * ra;
    Some((DVec3::new(sx, sy, sz), [shxy, shxz, shyz], q))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryNode {
    pub id: Id,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub params: Params,
    #[serde(skip)]
    pub out: Option<PolyMesh>,
    #[serde(skip)]
    pub error: Option<String>,
}

fn history_params(n: &crate::graph::DepNode) -> Params {
    n.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Lightweight construction-history entry for lists and tabs (no attribute maps).
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryMeta {
    pub id: Id,
    pub name: String,
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshShape {
    pub material: Id,
    pub smooth_level: u32,
    pub nurbs: bool,
    /// Dependency-node ids of the construction-history chain, first input to last output.
    pub history: Vec<Id>,
    pub mesh: PolyMesh,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightShape {
    pub ty: String,
    pub attrs: Params,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CamShape {
    pub focal_length: f64,
    pub near: f64,
    pub far: f64,
    pub ortho: bool,
    pub ortho_width: f64,
    pub coi: f64,
    pub horizontal_film_aperture: f64,
    pub vertical_film_aperture: f64,
    #[serde(default)]
    pub startup: bool,
}

impl Default for CamShape {
    fn default() -> Self {
        CamShape { focal_length: 35.0, near: 0.1, far: 10000.0, ortho: false, ortho_width: 30.0, coi: 10.0, horizontal_film_aperture: 1.417, vertical_film_aperture: 0.945, startup: false }
    }
}

impl CamShape {
    /// Vertical field of view (radians) for an aspect ratio, Maya's horizontal film fit.
    pub fn fov_y(&self, aspect: f64) -> f64 {
        let hfov = 2.0 * ((self.horizontal_film_aperture * 25.4 / 2.0) / self.focal_length).atan();
        2.0 * ((hfov / 2.0).tan() / aspect.max(1e-6)).atan()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Group,
    Locator,
    Mesh(Box<MeshShape>),
    Light(LightShape),
    Camera(CamShape),
    Curve(Curve),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: Id,
    pub name: String,
    /// "mesh" | "group" | "locator" | "light" | "camera" | "curve" | (rig kinds from the JS app)
    pub kind: String,
    pub parent: Option<Id>,
    pub children: Vec<Id>,
    pub xf: Xform,
    pub visible: bool,
    pub layer: Option<Id>,
    pub template: bool,
    /// can be picked and selected (Attribute Editor > Node Behavior)
    pub selectable: bool,
    /// per-object x-ray (Attribute Editor > Render Stats)
    pub xray: bool,
    /// Maya's Use Outliner Color / Outliner Color attributes.
    pub use_outliner_color: bool,
    pub outliner_color: [f32; 3],
    pub locks: Map<String, Value>,
    /// Attributes removed from the Channel Box by Hide Selected / Lock and Hide.
    pub hidden_channels: BTreeSet<String>,
    /// Attributes that cannot be keyframed (Channel Box > Make Nonkeyable).
    pub nonkeyable: BTreeSet<String>,
    pub shape_name: Option<String>,
    pub extra: Value,
    /// Per-channel expressions: attribute name -> expression source.
    pub expressions: IndexMap<String, String>,
    pub shape: Shape,
    /// data we don't interpret yet (rigging) but keep so files round-trip
    pub passthrough: Map<String, Value>,
    /// Maya-style published interface attributes: public name -> bound plug
    pub published: IndexMap<String, PublishedAttr>,
    /// bumped whenever the shape's geometry changes (GPU caches key on it)
    pub rev: u64,
}

/// A published attribute on a container: the public name lives in `Node::published`,
/// this record points at the internal plug it exposes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PublishedAttr {
    /// "dag" | "material" | "texture" | "unbound"
    pub kind: String,
    pub node: Id,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub index: usize,
    pub attr: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Node {
    pub fn startup(&self) -> bool {
        matches!(&self.shape, Shape::Camera(c) if c.startup)
    }
    pub fn mesh(&self) -> Option<&MeshShape> {
        match &self.shape {
            Shape::Mesh(m) => Some(m),
            _ => None,
        }
    }
    pub fn mesh_mut(&mut self) -> Option<&mut MeshShape> {
        match &mut self.shape {
            Shape::Mesh(m) => Some(m),
            _ => None,
        }
    }
    pub fn curve(&self) -> Option<&crate::curves::Curve> {
        match &self.shape {
            Shape::Curve(c) => Some(c),
            _ => None,
        }
    }
    pub fn curve_mut(&mut self) -> Option<&mut crate::curves::Curve> {
        match &mut self.shape {
            Shape::Curve(c) => Some(c),
            _ => None,
        }
    }
    pub fn is_locked(&self, attr: &str) -> bool {
        self.locks.get(attr).and_then(|v| v.as_bool()).unwrap_or(false)
    }
    pub fn channel_hidden(&self, attr: &str) -> bool {
        self.hidden_channels.contains(attr)
    }
    pub fn is_keyable(&self, attr: &str) -> bool {
        !self.nonkeyable.contains(attr)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: Id,
    pub name: String,
    #[serde(default = "yes")]
    pub visible: bool,
    /// "normal" | "template" | "reference"
    #[serde(default = "normal")]
    pub mode: String,
    #[serde(default)]
    pub color: Option<Value>,
    /// Visible during playback (the `P` column).
    #[serde(default = "yes")]
    pub playback: bool,
    /// Layer overrides are applied; off makes the layer behave as if unassigned.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Draw objects shaded (off = wireframe).
    #[serde(default = "yes")]
    pub shading: bool,
    /// Apply textures in Textured mode.
    #[serde(default = "yes")]
    pub texturing: bool,
    /// "full" | "bbox"
    #[serde(default = "full_lod")]
    pub lod: String,
    /// Animate objects in the layer during playback.
    #[serde(default = "yes")]
    pub anim_playback: bool,
    /// Creation order, used by Sort Layers Chronologically (0 for older files).
    #[serde(default)]
    pub created: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
fn yes() -> bool {
    true
}
fn normal() -> String {
    "normal".into()
}
fn full_lod() -> String {
    "full".into()
}

fn add_display_specs(n: &mut crate::graph::DepNode) {
    n.add_attr("visibility", crate::graph::AttrSpec::boolean(true));
    n.add_attr("displayType", crate::graph::AttrSpec::enumeration(&["normal", "template", "reference"], "normal"));
    n.add_attr("colorSet", crate::graph::AttrSpec::boolean(false));
    n.add_attr("color", crate::graph::AttrSpec::vector([0.5, 0.5, 0.5]));
    n.add_attr("playback", crate::graph::AttrSpec::boolean(true));
    n.add_attr("enabled", crate::graph::AttrSpec::boolean(true));
    n.add_attr("shading", crate::graph::AttrSpec::boolean(true));
    n.add_attr("texturing", crate::graph::AttrSpec::boolean(true));
    n.add_attr("lod", crate::graph::AttrSpec::enumeration(&["full", "bbox"], "full"));
    n.add_attr("animPlayback", crate::graph::AttrSpec::boolean(true));
    n.add_attr("layerManager", crate::graph::AttrSpec::message());
}

impl Layer {
    /// Clear the pre-node fields after their values have moved into a `displayLayer` node.
    fn reset_legacy_state(&mut self) {
        self.visible = true;
        self.mode = "normal".into();
        self.color = None;
        self.playback = true;
        self.enabled = true;
        self.shading = true;
        self.texturing = true;
        self.lod = "full".into();
        self.anim_playback = true;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeState {
    pub current: f64,
    pub start: f64,
    pub end: f64,
    pub anim_start: f64,
    pub anim_end: f64,
    pub fps: f64,
    #[serde(default)]
    pub playing: bool,
    #[serde(default = "continuous")]
    pub r#loop: String,
    #[serde(default)]
    pub auto_key: bool,
    #[serde(default)]
    pub play_every: bool,
}
fn continuous() -> String {
    "continuous".into()
}
impl Default for TimeState {
    fn default() -> Self {
        TimeState { current: 1.0, start: 1.0, end: 120.0, anim_start: 1.0, anim_end: 200.0, fps: 24.0, playing: false, r#loop: continuous(), auto_key: false, play_every: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompMode {
    Vertex,
    Edge,
    Face,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompSel {
    pub v: BTreeSet<u32>,
    pub e: BTreeSet<(u32, u32)>,
    pub f: BTreeSet<usize>,
}

impl CompSel {
    pub fn is_empty(&self) -> bool {
        self.v.is_empty() && self.e.is_empty() && self.f.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MsgKind {
    Echo,
    Result,
    Warning,
    Error,
    Info,
}

/// A direct attribute connection (connectAttr).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conn {
    pub src: Id,
    #[serde(rename = "srcAttr")]
    pub src_attr: String,
    pub dst: Id,
    #[serde(rename = "dstAttr")]
    pub dst_attr: String,
}

/// A transform constraint (point / orient / parent / scale / aim).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: Id,
    pub name: String,
    pub kind: String,
    pub driven: Id,
    pub targets: Vec<Id>,
    /// maintained offset (column-major matrix)
    #[serde(default)]
    pub offset: Option<[f64; 16]>,
    #[serde(default = "aim_x")]
    pub aim: [f64; 3],
    #[serde(default = "up_y")]
    pub up: [f64; 3],
}
fn aim_x() -> [f64; 3] {
    [1.0, 0.0, 0.0]
}
fn up_y() -> [f64; 3] {
    [0.0, 1.0, 0.0]
}

/// What a name refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeRef {
    Dag(Id),
    History(Id, usize),
    Material(Id),
    Texture(Id),
    Place2D(Id),
    /// A non-DAG node in `Scene::deps` (animation layer, shading node, utility node).
    Dependency(Id),
}

#[derive(Clone)]
pub struct Snapshot {
    pub label: String,
    pub data: Value,
    /// Only the selection/mode/hilite fields need restoring; the scene itself is unchanged.
    pub selection_only: bool,
}

pub struct Scene {
    pub nodes: IndexMap<Id, Node>,
    /// Non-DAG dependency nodes (layers, shading nodes, utility nodes).
    pub deps: crate::graph::DepGraph,
    pub roots: Vec<Id>,
    pub names: HashSet<String>,
    pub default_mat: Id,
    pub layers: Vec<Layer>,
    /// The layer new objects are assigned to when `add_new_to_current` is on.
    pub current_layer: Option<Id>,
    /// Layer Editor option: assign newly created objects to the current layer.
    pub add_new_to_current: bool,
    /// Playback state mirrored from the app so `is_visible` can apply layer `P` flags.
    pub playing: bool,
    pub anim: Anim,
    /// Animation layers above the base animation (top of the list first).
    pub anim_layers: Vec<AnimLayer>,
    /// Animation layer receiving new keys; None = BaseAnimation.
    pub anim_layer_current: Option<Id>,
    /// BaseAnimation refuses new keys while locked.
    pub anim_base_lock: bool,
    /// Draw BaseAnimation alone as a ghost.
    pub anim_base_ghost: bool,
    pub anim_base_ghost_color: [f32; 3],
    /// Mute/lock state of the layers before Solo; restored when Solo is cleared.
    pub anim_solo_saved: Vec<(Id, bool, bool)>,
    /// Static channel values animation layers blend against when BaseAnimation has no curve.
    pub anim_static: IndexMap<String, f64>,
    pub conns: Vec<Conn>,
    pub constraints: Vec<Constraint>,
    /// HumanIK character definitions
    pub characters: Vec<crate::hik::Character>,
    pub time: TimeState,
    pub render: Params,
    pub sel: Vec<Id>,
    /// Component-mode hilite objects (JS `App.hilite`): the objects whose components are shown
    /// and edited. Kept separate from `sel` because the JS object selection does not change it.
    pub hilite: Vec<Id>,
    pub comp_mode: Option<CompMode>,
    pub comp: HashMap<Id, CompSel>,
    pub id_counter: u64,
    pub project: Option<String>,
    pub file: Option<std::path::PathBuf>,
    pub modified: bool,
    pub msgs: Vec<(MsgKind, String)>,
    /// Node renames from the last command batch, consumed by the UI to fix name references.
    pub name_changes: Vec<(String, String)>,
    pub undo_stack: Vec<Snapshot>,
    pub redo_stack: Vec<Snapshot>,
    pub undo_limit: usize,
    /// default in / out tangents for new keys (Preferences > Animation)
    pub key_tangents: (String, String),
    /// bumped on every change (UI caches)
    pub rev: u64,
    pub version: String,
    /// simulation state (not saved)
    pub dynamics: crate::dynamics::Dynamics,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

fn shape_name_for(n: &str) -> String {
    let digits = n.len() - n.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 {
        format!("{}Shape{}", &n[..n.len() - digits], &n[n.len() - digits..])
    } else {
        format!("{n}Shape")
    }
}

impl Scene {
    pub fn new() -> Self {
        let mut s = Scene {
            nodes: IndexMap::new(),
            deps: crate::graph::DepGraph::default(),
            roots: vec![],
            names: HashSet::new(),
            default_mat: String::new(),
            layers: vec![],
            current_layer: None,
            add_new_to_current: false,
            playing: false,
            anim: Anim::default(),
            anim_layers: vec![],
            anim_layer_current: None,
            anim_base_lock: false,
            anim_base_ghost: false,
            anim_base_ghost_color: crate::anim::ghost_red(),
            anim_solo_saved: vec![],
            anim_static: IndexMap::new(),
            conns: vec![],
            constraints: vec![],
            characters: vec![],
            time: TimeState::default(),
            render: Params::new(),
            sel: vec![],
            hilite: vec![],
            comp_mode: None,
            comp: HashMap::new(),
            id_counter: 1,
            project: None,
            file: None,
            modified: false,
            msgs: vec![],
            name_changes: vec![],
            undo_stack: vec![],
            redo_stack: vec![],
            undo_limit: 100,
            key_tangents: ("auto".into(), "auto".into()),
            rev: 0,
            version: env!("CARGO_PKG_VERSION").into(),
            dynamics: Default::default(),
        };
        s.init_default_materials();
        s.create_startup_cameras();
        s.ensure_default_display_layer();
        s.ensure_layer_manager();
        s.ensure_display_layer_nodes();
        s
    }

    /// Maya always has a bottom `defaultLayer`; objects without an explicit display layer belong
    /// to it.
    pub fn ensure_default_display_layer(&mut self) {
        if !self.layers.iter().any(|l| l.id == "defaultLayer") {
            self.layers.push(Layer {
                id: "defaultLayer".into(),
                name: "defaultLayer".into(),
                visible: true,
                mode: "normal".into(),
                color: None,
                playback: true,
                enabled: true,
                shading: true,
                texturing: true,
                lod: "full".into(),
                anim_playback: true,
                created: 0,
                extra: Default::default(),
            });
        }
        if let Some(pos) = self.layers.iter().position(|l| l.id == "defaultLayer") {
            if pos + 1 < self.layers.len() {
                let l = self.layers.remove(pos);
                self.layers.push(l);
            }
        }
        if self.current_layer.as_ref().is_none_or(|id| !self.layers.iter().any(|l| &l.id == id)) {
            self.current_layer = Some("defaultLayer".into());
        }
    }

    /// Change the display layer new objects are added to and mirror it on `layerManager`.
    pub fn set_current_display_layer(&mut self, id: &str) {
        if !self.layers.iter().any(|l| l.id == id) {
            return;
        }
        self.current_layer = Some(id.to_string());
        self.ensure_layer_manager();
        let _ = self.deps.set(&crate::graph::PlugRef::new("layerManager", "currentLayer"), json!(id));
        self.touch();
    }

    /// Maya's singleton `layerManager` node. Display layers connect their `layerManager`
    /// input to its `displayLayers` output.
    pub fn ensure_layer_manager(&mut self) -> Id {
        if self.deps.node("layerManager").is_none() {
            let mut n = crate::graph::DepNode::new("layerManager", "layerManager", "layerManager");
            n.add_attr("displayLayers", crate::graph::AttrSpec::message());
            n.add_attr("currentLayer", crate::graph::AttrSpec::string("defaultLayer"));
            self.deps.insert(n);
            self.names.insert("layerManager".into());
        } else if let Some(n) = self.deps.node_mut("layerManager") {
            if n.name != "layerManager" {
                self.names.remove(&n.name);
                n.name = "layerManager".into();
                self.names.insert("layerManager".into());
            }
            if n.spec("displayLayers").is_none() {
                n.add_attr("displayLayers", crate::graph::AttrSpec::message());
            }
            if n.spec("currentLayer").is_none() {
                n.add_attr("currentLayer", crate::graph::AttrSpec::string("defaultLayer"));
            }
        }
        if let Some(current) = self.current_layer.clone() {
            let _ = self.deps.set(&crate::graph::PlugRef::new("layerManager", "currentLayer"), json!(current));
        }
        "layerManager".into()
    }

    // ------------------------------------------------------------ ids & names
    pub fn new_id(&mut self, prefix: &str) -> Id {
        let id = format!("{prefix}{}", self.id_counter);
        self.id_counter += 1;
        id
    }

    pub fn unique_name(&mut self, want: &str) -> String {
        let mut n: String = want.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == ':' { c } else { '_' }).collect();
        if n.is_empty() {
            n = "node".into();
        }
        if n.starts_with(|c: char| c.is_ascii_digit()) {
            n.insert(0, '_');
        }
        if !self.names.contains(&n) {
            self.names.insert(n.clone());
            return n;
        }
        let digits = n.len() - n.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (stem, mut i) = if digits > 0 { (n[..n.len() - digits].to_string(), n[n.len() - digits..].parse::<u64>().unwrap_or(1)) } else { (n.clone(), 1) };
        while self.names.contains(&format!("{stem}{i}")) {
            i += 1;
        }
        let r = format!("{stem}{i}");
        self.names.insert(r.clone());
        r
    }
    pub fn release_name(&mut self, n: &str) {
        self.names.remove(n);
    }

    pub fn msg(&mut self, k: MsgKind, s: impl Into<String>) {
        self.msgs.push((k, s.into()));
    }
    pub fn echo(&mut self, s: impl Into<String>) {
        self.msg(MsgKind::Echo, s)
    }
    pub fn warn(&mut self, s: impl Into<String>) {
        self.msg(MsgKind::Warning, format!("// Warning: {}", s.into()))
    }

    pub fn touch(&mut self) {
        self.rev += 1;
        self.modified = true;
    }

    // ------------------------------------------------------------ materials
    pub fn init_default_materials(&mut self) {
        let lam = self.create_material("lambert", Some("lambert1"));
        self.default_mat = lam;
        if let Some(sg) = self.shading_group_of(&self.default_mat) {
            self.rename_shading_node(&sg, "initialShadingGroup");
        }
        self.create_material("standardSurface", Some("standardSurface1"));
    }

    pub fn is_material(&self, id: &str) -> bool {
        self.deps.node(id).is_some_and(|n| crate::materials::is_material_kind(&n.kind))
    }
    pub fn is_shading_engine(&self, id: &str) -> bool {
        self.deps.node(id).is_some_and(|n| n.kind == crate::materials::SHADING_ENGINE_KIND)
    }
    /// The shading group connected from this material's outColor.
    pub fn shading_group_of(&self, material_id: &str) -> Option<Id> {
        self.deps.nodes.values().find(|n| {
            n.kind == crate::materials::SHADING_ENGINE_KIND
                && self.deps.source_of(&crate::graph::PlugRef::new(&n.id, "surfaceShader")).is_some_and(|p| p.node == material_id)
        }).map(|n| n.id.clone())
    }
    /// The material feeding a shading group's surfaceShader input.
    pub fn shading_group_material(&self, sg_id: &str) -> Option<Id> {
        self.deps.source_of(&crate::graph::PlugRef::new(sg_id, "surfaceShader")).map(|p| p.node).filter(|id| self.is_material(id))
    }

    /// Maya always gives a material a shading group; create it once and connect the shader.
    pub fn ensure_shading_engine(&mut self, material_id: &str) -> Id {
        if let Some(sg) = self.shading_group_of(material_id) {
            return sg;
        }
        let name = self.material(material_id).map(|m| format!("{}SG", m.name)).unwrap_or_else(|| "shadingEngine1".into());
        let id = format!("g_{material_id}");
        let name = self.unique_name(&name);
        let mut n = crate::graph::DepNode::new(&id, &name, crate::materials::SHADING_ENGINE_KIND);
        crate::materials::add_shading_engine_specs(&mut n);
        self.deps.insert(n);
        self.deps.connect(crate::graph::PlugRef::new(material_id, "outColor"), crate::graph::PlugRef::new(&id, "surfaceShader"));
        id
    }
    pub fn is_texture(&self, id: &str) -> bool {
        self.deps.node(id).is_some_and(|n| crate::materials::is_texture_kind(&n.kind))
    }
    pub fn is_place(&self, id: &str) -> bool {
        self.deps.node(id).is_some_and(|n| n.kind == crate::materials::PLACE_KIND)
    }
    pub fn is_shading(&self, id: &str) -> bool {
        self.is_material(id) || self.is_texture(id) || self.is_place(id)
    }
    pub fn material_ids(&self) -> Vec<Id> {
        self.deps.nodes.values().filter(|n| crate::materials::is_material_kind(&n.kind)).map(|n| n.id.clone()).collect()
    }
    pub fn texture_ids(&self) -> Vec<Id> {
        self.deps.nodes.values().filter(|n| crate::materials::is_texture_kind(&n.kind)).map(|n| n.id.clone()).collect()
    }
    pub fn place_ids(&self) -> Vec<Id> {
        self.deps.nodes.values().filter(|n| n.kind == crate::materials::PLACE_KIND).map(|n| n.id.clone()).collect()
    }
    /// Texture nodes mapped into any material slot.
    pub fn material_textures(&self, id: &str) -> Vec<Id> {
        self.material(id).map(|m| m.maps.values().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }
    /// Materials that map this texture into one of their slots.
    pub fn texture_owners(&self, id: &str) -> Vec<Id> {
        self.materials().into_iter().filter(|m| m.maps.values().any(|v| v.as_str() == Some(id))).map(|m| m.id).collect()
    }

    /// Shading nodes are dependency nodes; this keeps `NodeRef` variants for the UI.
    pub fn shading_ref(&self, id: &str) -> Option<NodeRef> {
        let n = self.deps.node(id)?;
        if crate::materials::is_material_kind(&n.kind) {
            Some(NodeRef::Material(id.into()))
        } else if crate::materials::is_texture_kind(&n.kind) {
            Some(NodeRef::Texture(id.into()))
        } else if n.kind == crate::materials::PLACE_KIND {
            Some(NodeRef::Place2D(id.into()))
        } else {
            Some(NodeRef::Dependency(id.into()))
        }
    }

    /// Material attributes and texture maps as a read-only view over its dependency node.
    pub fn material(&self, id: &str) -> Option<Material> {
        let n = self.deps.node(id)?;
        if !crate::materials::is_material_kind(&n.kind) {
            return None;
        }
        let mut m = Material { id: id.into(), name: n.name.clone(), ty: n.kind.clone(), attrs: Params::new(), maps: Params::new(), map_outs: Params::new() };
        for (k, v) in &n.attrs {
            let keep = !matches!(k.as_str(), "outTransparency" | "outColor") || m.ty == "surfaceShader";
            if keep && n.spec(k).is_some() {
                m.attrs.insert(k.clone(), v.clone());
            }
        }
        for c in &self.deps.connections {
            if c.destination.node == id && self.is_texture(&c.source.node) {
                m.maps.insert(c.destination.attr.clone(), json!(c.source.node));
                if c.source.attr == "outAlpha" {
                    m.map_outs.insert(c.destination.attr.clone(), json!("outAlpha"));
                }
            }
        }
        Some(m)
    }

    pub fn materials(&self) -> Vec<Material> {
        self.deps.nodes.values().filter(|n| crate::materials::is_material_kind(&n.kind)).filter_map(|n| self.material(&n.id)).collect()
    }

    pub fn material_named(&self, name: &str) -> Option<Id> {
        self.deps.nodes.values().find(|n| n.name == name && crate::materials::is_material_kind(&n.kind)).map(|n| n.id.clone())
    }

    /// Texture attributes and its connected place2dTexture, as a read-only view.
    pub fn texture(&self, id: &str) -> Option<Texture> {
        let n = self.deps.node(id)?;
        if !crate::materials::is_texture_kind(&n.kind) {
            return None;
        }
        let mut t = Texture { id: id.into(), name: n.name.clone(), ty: n.kind.clone(), attrs: Params::new(), place: None };
        for (k, v) in &n.attrs {
            if !matches!(k.as_str(), "uvCoord" | "outColor" | "outAlpha") {
                t.attrs.insert(k.clone(), v.clone());
            }
        }
        t.place = self
            .deps
            .source_of(&crate::graph::PlugRef::new(id, "uvCoord"))
            .filter(|p| p.attr == "outUV" && self.is_place(&p.node))
            .map(|p| p.node);
        Some(t)
    }

    pub fn textures(&self) -> Vec<Texture> {
        self.deps.nodes.values().filter(|n| crate::materials::is_texture_kind(&n.kind)).filter_map(|n| self.texture(&n.id)).collect()
    }

    pub fn place(&self, id: &str) -> Option<Place2D> {
        let n = self.deps.node(id)?;
        if n.kind != crate::materials::PLACE_KIND {
            return None;
        }
        let mut p = Place2D { id: id.into(), name: n.name.clone(), attrs: Params::new() };
        for (k, v) in &n.attrs {
            if k != "outUV" {
                p.attrs.insert(k.clone(), v.clone());
            }
        }
        Some(p)
    }

    pub fn places(&self) -> Vec<Place2D> {
        self.deps.nodes.values().filter(|n| n.kind == crate::materials::PLACE_KIND).filter_map(|n| self.place(&n.id)).collect()
    }

    pub fn create_material(&mut self, ty: &str, name: Option<&str>) -> Id {
        let id = self.new_id("m");
        let name = self.unique_name(name.unwrap_or(&format!("{ty}1")));
        let mut n = crate::graph::DepNode::new(&id, &name, ty);
        crate::materials::add_material_specs(&mut n, ty);
        self.deps.insert(n);
        self.ensure_shading_engine(&id);
        self.touch();
        id
    }

    fn create_place_node(&mut self, placement: &Params) -> Id {
        let id = self.new_id("p");
        let name = self.unique_name("place2dTexture1");
        let mut n = crate::graph::DepNode::new(&id, &name, crate::materials::PLACE_KIND);
        crate::materials::add_place_specs(&mut n, placement);
        self.deps.insert(n);
        id
    }

    pub fn create_texture(&mut self, ty: &str, name: Option<&str>) -> Id {
        let id = self.new_id("x");
        let name = self.unique_name(name.unwrap_or(&format!("{ty}1")));
        let mut n = crate::graph::DepNode::new(&id, &name, ty);
        crate::materials::add_texture_specs(&mut n, ty);
        self.deps.insert(n);
        let place = self.create_place_node(&Place2D::new(String::new(), String::new(), ty).attrs);
        self.deps.connect(crate::graph::PlugRef::new(&place, "outUV"), crate::graph::PlugRef::new(&id, "uvCoord"));
        self.touch();
        id
    }

    /// Standalone place2dTexture utility (not owned by a texture yet).
    pub fn create_place(&mut self, name: Option<&str>) -> Id {
        let id = self.create_place_node(&Place2D::new(String::new(), String::new(), "").attrs);
        if let Some(name) = name {
            let old = self.deps.node(&id).map(|n| n.name.clone()).unwrap_or_default();
            self.names.remove(&old);
            let want = self.unique_name(name);
            if let Some(n) = self.deps.node_mut(&id) {
                n.name = want.clone();
            }
            self.names.insert(want);
        }
        self.touch();
        id
    }

    pub fn place_of(&self, t: &Texture) -> Option<Place2D> {
        self.place(t.place.as_deref()?)
    }

    /// Store a shading attribute, adding a spec for attributes that came from older files.
    pub fn set_dep_attr(&mut self, id: &str, attr: &str, value: Value) -> bool {
        let Some(n) = self.deps.node_mut(id) else { return false };
        if n.spec(attr).is_none() {
            n.add_attr(attr, crate::graph::AttrSpec::any(value.clone()));
        }
        let ok = n.set(attr, value);
        if ok {
            self.touch();
        }
        ok
    }

    /// Convert the pre-node `shading` block into dependency nodes, keeping ids and names.
    fn migrate_legacy_shading(&mut self, shading: ShadingData) {
        let mut places: HashSet<Id> = HashSet::new();
        for mut p in shading.places {
            crate::materials::add_placement_defaults(&mut p.attrs);
            let mut n = crate::graph::DepNode::new(&p.id, &p.name, crate::materials::PLACE_KIND);
            crate::materials::add_place_specs(&mut n, &p.attrs);
            for (k, v) in &p.attrs {
                n.set(k, v.clone());
            }
            self.names.insert(p.name.clone());
            places.insert(p.id.clone());
            self.deps.insert(n);
        }
        for mut t in shading.texs {
            let legacy = crate::materials::take_placement_attrs(&mut t.attrs);
            let pid = match t.place.clone().filter(|id| places.contains(id)) {
                Some(id) => id,
                None => {
                    let id = self.new_id("p");
                    let name = self.unique_name("place2dTexture1");
                    let attrs = Place2D::new(id.clone(), name.clone(), &t.ty).attrs;
                    let mut n = crate::graph::DepNode::new(&id, &name, crate::materials::PLACE_KIND);
                    crate::materials::add_place_specs(&mut n, &attrs);
                    self.deps.insert(n);
                    places.insert(id.clone());
                    id
                }
            };
            if let Some(p) = self.deps.node_mut(&pid) {
                for (k, v) in legacy {
                    if p.spec(&k).is_none() {
                        p.add_attr(&k, crate::graph::AttrSpec::any(v.clone()));
                    }
                    p.set(&k, v);
                }
            }
            let mut n = crate::graph::DepNode::new(&t.id, &t.name, &t.ty);
            crate::materials::add_texture_specs(&mut n, &t.ty);
            for (k, v) in &t.attrs {
                if n.spec(k).is_none() {
                    n.add_attr(k, crate::graph::AttrSpec::any(v.clone()));
                }
                n.set(k, v.clone());
            }
            self.names.insert(t.name.clone());
            self.deps.insert(n);
            self.deps.connect(crate::graph::PlugRef::new(&pid, "outUV"), crate::graph::PlugRef::new(&t.id, "uvCoord"));
        }
        for m in shading.mats {
            let mut n = crate::graph::DepNode::new(&m.id, &m.name, &m.ty);
            crate::materials::add_material_specs(&mut n, &m.ty);
            for (k, v) in &m.attrs {
                if n.spec(k).is_none() {
                    n.add_attr(k, crate::graph::AttrSpec::any(v.clone()));
                }
                n.set(k, v.clone());
            }
            self.names.insert(m.name.clone());
            self.deps.insert(n);
            for (slot, tex) in &m.maps {
                let Some(tex) = tex.as_str() else { continue };
                let out = m.map_out(slot);
                self.deps.connect(crate::graph::PlugRef::new(tex, out), crate::graph::PlugRef::new(&m.id, slot));
            }
            self.ensure_shading_engine(&m.id);
        }
    }

    /// Connect a texture output to a material slot (`None` output means outColor).
    pub fn set_material_map(&mut self, mat: &str, slot: &str, tex: &str, out: &str) -> bool {
        if !self.is_material(mat) || !self.is_texture(tex) || self.deps.node(mat).is_none_or(|n| n.spec(slot).is_none()) {
            return false;
        }
        let dst = crate::graph::PlugRef::new(mat, slot);
        let src = crate::graph::PlugRef::new(tex, if out == "outAlpha" { "outAlpha" } else { "outColor" });
        let ok = self.deps.connect(src, dst);
        if ok {
            self.touch();
        }
        ok
    }

    pub fn clear_material_map(&mut self, mat: &str, slot: &str) -> bool {
        let ok = self.deps.disconnect(&crate::graph::PlugRef::new(mat, slot));
        if ok {
            self.touch();
        }
        ok
    }

    /// Rename a shading node; returns the new unique name.
    pub fn rename_shading_node(&mut self, id: &str, want: &str) -> Option<Id> {
        let old = self.deps.node(id)?.name.clone();
        if self.is_material(id) || self.is_texture(id) || self.is_place(id) || self.is_shading_engine(id) {
            if old == want {
                return Some(id.to_string());
            }
            self.names.remove(&old);
            let name = self.unique_name(want);
            self.deps.node_mut(id)?.name = name.clone();
            self.touch();
            Some(name)
        } else {
            None
        }
    }

    /// Remove any shading dependency node, releasing its name.
    pub fn delete_shading_node(&mut self, id: &str) {
        if let Some(n) = self.deps.remove(id) {
            self.names.remove(&n.name);
            self.touch();
        }
    }

    pub fn delete_texture(&mut self, id: &str) {
        let place = self.deps.source_of(&crate::graph::PlugRef::new(id, "uvCoord")).map(|p| p.node);
        if let Some(n) = self.deps.remove(id) {
            self.names.remove(&n.name);
        }
        if let Some(pid) = place {
            let used = self.deps.nodes.values().any(|n| {
                crate::materials::is_texture_kind(&n.kind)
                    && self.deps.source_of(&crate::graph::PlugRef::new(&n.id, "uvCoord")).is_some_and(|p| p.node == pid)
            });
            if !used {
                if let Some(p) = self.deps.remove(&pid) {
                    self.names.remove(&p.name);
                }
            }
        }
        self.touch();
    }

    pub fn assign_material(&mut self, node: &str, mat: &str, faces: Option<&[usize]>) {
        if let Some(n) = self.nodes.get_mut(node) {
            if let Some(m) = n.mesh_mut() {
                match faces {
                    None => {
                        m.material = mat.into();
                        m.mesh.fm = None;
                        bake_fm(m);
                    }
                    Some(fs) => {
                        let nf = m.mesh.f.len();
                        let fm = m.mesh.fm.get_or_insert_with(|| vec![None; nf]);
                        fm.resize(nf, None);
                        for &f in fs {
                            if f < nf {
                                fm[f] = if mat == m.material { None } else { Some(mat.to_string()) };
                            }
                        }
                        bake_fm(m);
                    }
                }
                n.rev += 1;
            }
        }
        self.touch();
    }

    pub fn delete_material(&mut self, id: &str) {
        if id == self.default_mat || self.deps.node(id).is_some_and(|m| m.name == "standardSurface1") {
            return;
        }
        for sg in self.deps.nodes.values().filter(|n| {
            n.kind == crate::materials::SHADING_ENGINE_KIND
                && self.deps.source_of(&crate::graph::PlugRef::new(&n.id, "surfaceShader")).is_some_and(|p| p.node == id)
        }).map(|n| n.id.clone()).collect::<Vec<_>>() {
            if let Some(n) = self.deps.remove(&sg) {
                self.names.remove(&n.name);
            }
        }
        if let Some(m) = self.deps.remove(id) {
            self.names.remove(&m.name);
        }
        let def = self.default_mat.clone();
        for n in self.nodes.values_mut() {
            if let Some(m) = n.mesh_mut() {
                if m.material == id {
                    m.material = def.clone();
                }
                if let Some(fm) = &mut m.mesh.fm {
                    for x in fm.iter_mut() {
                        if x.as_deref() == Some(id) {
                            *x = None;
                        }
                    }
                }
                n.rev += 1;
            }
        }
        self.touch();
    }

    // ------------------------------------------------------------ DAG
    fn make_dag(&mut self, kind: &str, name: &str, parent: Option<&str>, shape: Shape, exact: bool, id: Option<Id>) -> Id {
        let id = id.unwrap_or_else(|| self.new_id("d"));
        let name = if exact {
            self.names.insert(name.to_string());
            name.to_string()
        } else {
            self.unique_name(name)
        };
        let shape_name = match &shape {
            Shape::Group => None,
            _ => Some(self.unique_name(&shape_name_for(&name))),
        };
        let layer = if self.add_new_to_current { self.current_layer.clone().filter(|id| id != "defaultLayer") } else { None };
        let node = Node {
            id: id.clone(),
            name,
            kind: kind.into(),
            parent: parent.map(String::from),
            children: vec![],
            xf: Xform::default(),
            visible: true,
            layer,
            template: false,
            selectable: true,
            xray: false,
            use_outliner_color: false,
            outliner_color: [1.0, 1.0, 1.0],
            locks: Map::new(),
            hidden_channels: BTreeSet::new(),
            nonkeyable: BTreeSet::new(),
            passthrough: Map::new(),
            shape_name,
            extra: Value::Null,
            expressions: IndexMap::new(),
            shape,
            published: IndexMap::new(),
            rev: 1,
        };
        match parent.and_then(|p| self.nodes.get_mut(p)) {
            Some(p) => p.children.push(id.clone()),
            None => self.roots.push(id.clone()),
        }
        self.nodes.insert(id.clone(), node);
        self.touch();
        id
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.touch();
        self.nodes.get_mut(id)
    }

    pub fn create_group(&mut self, name: &str, parent: Option<&str>) -> Id {
        self.make_dag("group", name, parent, Shape::Group, false, None)
    }
    /// A Maya-style container/asset node (a transform with a published interface).
    pub fn create_container(&mut self, name: &str, parent: Option<&str>) -> Id {
        self.make_dag("container", name, parent, Shape::Group, false, None)
    }
    pub fn create_locator(&mut self, name: &str) -> Id {
        self.make_dag("locator", name, None, Shape::Locator, false, None)
    }

    pub fn create_mesh(&mut self, gen: &str, params: &Params, name: Option<&str>) -> Id {
        let info = primitives::gen_info(gen);
        let base = name.map(String::from).unwrap_or_else(|| format!("{}1", info.map(|g| g.base).unwrap_or("polySurface")));
        let shape = Shape::Mesh(Box::new(MeshShape { material: self.default_mat.clone(), smooth_level: 0, nurbs: info.is_some_and(|g| g.nurbs), history: vec![], mesh: PolyMesh::default() }));
        let id = self.make_dag("mesh", &base, None, shape, false, None);
        let mut p = primitives::default_params(gen);
        for (k, v) in params {
            p.insert(k.clone(), v.clone());
        }
        self.add_history(&id, gen, p);
        id
    }

    /// A mesh without construction history (its data lives in a hidden meshData node).
    pub fn create_mesh_from(&mut self, pm: PolyMesh, name: &str, material: Option<&str>, parent: Option<&str>) -> Id {
        let mat = material.map(String::from).unwrap_or(self.default_mat.clone());
        let shape = Shape::Mesh(Box::new(MeshShape { material: mat, smooth_level: 0, nurbs: false, history: vec![], mesh: PolyMesh::default() }));
        let id = self.make_dag("mesh", name, parent, shape, false, None);
        self.bake_mesh(&id, pm);
        id
    }

    pub fn create_light(&mut self, ty: &str) -> Id {
        let attrs = match light_defaults(ty) {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let id = self.make_dag("light", &format!("{ty}1"), None, Shape::Light(LightShape { ty: ty.into(), attrs }), false, None);
        if ty == "directionalLight" || ty == "spotLight" {
            // JS `commands.js`: directional/spot lights start at [-45, 45, 0]
            self.nodes.get_mut(&id).unwrap().xf.r = [-45.0, 45.0, 0.0];
        }
        id
    }

    /// A deformer handle (drawn as a locator-like icon).
    pub fn create_handle(&mut self, kind: &str, name: &str) -> Id {
        let id = self.make_dag(kind, name, None, Shape::Locator, false, None);
        if let Some(n) = self.nodes.get_mut(&id) {
            n.shape_name = None;
        }
        id
    }

    pub fn create_camera(&mut self, name: &str, cam: CamShape, exact: bool) -> Id {
        self.make_dag("camera", name, None, Shape::Camera(cam), exact, None)
    }

    pub fn create_curve(&mut self, c: Curve, name: &str) -> Id {
        self.make_dag("curve", name, None, Shape::Curve(c), false, None)
    }

    pub fn create_startup_cameras(&mut self) {
        let mk = |s: &mut Scene, name: &str, t: [f64; 3], r: [f64; 3], cam: CamShape| {
            let id = s.create_camera(name, CamShape { startup: true, ..cam }, true);
            let n = s.nodes.get_mut(&id).unwrap();
            n.xf.t = t;
            n.xf.r = r;
        };
        mk(self, "persp", [28.0, 21.0, 28.0], [-27.938, 45.0, 0.0], CamShape { coi: 44.82, ..Default::default() });
        mk(self, "top", [0.0, 1000.1, 0.0], [-90.0, 0.0, 0.0], CamShape { ortho: true, coi: 1000.1, ..Default::default() });
        mk(self, "front", [0.0, 0.0, 1000.1], [0.0, 0.0, 0.0], CamShape { ortho: true, coi: 1000.1, ..Default::default() });
        mk(self, "side", [1000.1, 0.0, 0.0], [0.0, 90.0, 0.0], CamShape { ortho: true, coi: 1000.1, ..Default::default() });
    }

    pub fn find(&self, name: &str) -> Option<Id> {
        let name = name.trim_start_matches('|');
        let name = name.rsplit('|').next().unwrap_or(name);
        self.nodes.values().find(|n| n.name == name).or_else(|| self.nodes.values().find(|n| n.shape_name.as_deref() == Some(name))).map(|n| n.id.clone())
    }

    /// Create a generic non-DAG node. Attributes are registered by the caller.
    pub fn create_dep_node(&mut self, kind: &str, name: &str) -> Id {
        let id = self.new_id(kind);
        let name = self.unique_name(name);
        self.names.insert(name.clone());
        self.deps.insert(crate::graph::DepNode::new(&id, &name, kind));
        self.touch();
        id
    }

    pub fn dep_node(&self, id: &str) -> Option<&crate::graph::DepNode> {
        self.deps.node(id)
    }

    pub fn dep_node_mut(&mut self, id: &str) -> Option<&mut crate::graph::DepNode> {
        self.deps.node_mut(id)
    }

    pub fn remove_dep_node(&mut self, id: &str) -> bool {
        let Some(n) = self.deps.remove(id) else { return false };
        self.names.remove(&n.name);
        self.touch();
        true
    }

    /// Make sure every animation layer has its real `animLayer` dependency node, and that every
    /// layered channel has its `animBlendNode` and `animCurve` nodes (Maya's layer network).
    pub fn ensure_anim_layer_nodes(&mut self) {
        let layers: Vec<(String, String)> = self.anim_layers.iter().map(|l| (l.id.clone(), l.name.clone())).collect();
        for (id, name) in layers {
            self.ensure_anim_layer_node(&id, &name);
        }
        self.migrate_legacy_layer_curves();
        let layers: Vec<(String, String, Vec<String>)> = self.anim_layers.iter().map(|l| (l.id.clone(), l.name.clone(), l.attrs.clone())).collect();
        for (lid, lname, attrs) in &layers {
            for key in attrs {
                self.ensure_anim_blend_node(lid, lname, key, false);
            }
        }
        let mut expected = std::collections::HashSet::new();
        for (lid, _, attrs) in &layers {
            for key in attrs {
                expected.insert(Self::anim_layer_blend_id(lid, key));
                if self.anim_layer_curve(lid, key).is_some_and(|c| !c.keys.is_empty()) {
                    expected.insert(Self::anim_layer_curve_id(lid, key));
                }
            }
        }
        // Chain each channel's blend nodes bottom-up: lower layer output -> inputA.
        for (lid, _, attrs) in &layers {
            let Some(i) = self.anim_layers.iter().position(|l| &l.id == lid) else { continue };
            for key in attrs {
                let bid = Self::anim_layer_blend_id(lid, key);
                let cid = Self::anim_layer_curve_id(lid, key);
                let curve_out = crate::graph::PlugRef::new(&bid, "inputB");
                if self.deps.node(&cid).is_some() {
                    let input_b = crate::graph::PlugRef::new(&cid, "output");
                    if self.deps.source_of(&curve_out) != Some(input_b.clone()) {
                        self.deps.disconnect(&curve_out);
                        self.deps.connect(input_b, curve_out);
                    }
                } else {
                    self.deps.disconnect(&curve_out);
                }
                let input_a = crate::graph::PlugRef::new(&bid, "inputA");
                let below = self.anim_layers.iter().skip(i + 1).find(|l| l.attrs.contains(key)).map(|l| Self::anim_layer_blend_id(&l.id, key));
                match below {
                    Some(lower) => {
                        let src = crate::graph::PlugRef::new(&lower, "output");
                        let dest = crate::graph::PlugRef::new(&bid, "inputA");
                        if self.deps.source_of(&dest) != Some(src.clone()) {
                            self.deps.disconnect(&dest);
                            self.deps.connect(src, dest);
                        }
                    }
                    None => {
                        self.deps.disconnect(&input_a);
                    }
                }
            }
        }
        let orphans: Vec<String> = self
            .deps
            .nodes
            .values()
            .filter(|n| (n.id.starts_with("ab|") || n.id.starts_with("ac|")) && !expected.contains(&n.id))
            .map(|n| n.id.clone())
            .collect();
        for id in orphans {
            self.remove_dep_node(&id);
        }
    }

    /// Move curves from the pre-node layer storage into their `animCurve` nodes.
    fn migrate_legacy_layer_curves(&mut self) {
        let mut pending = vec![];
        for l in &mut self.anim_layers {
            if l.curves.is_empty() {
                continue;
            }
            let curves = std::mem::take(&mut l.curves);
            for (key, curve) in curves {
                if !l.attrs.contains(&key) {
                    l.attrs.push(key.clone());
                }
                l.attrs.sort();
                l.attrs.dedup();
                pending.push((l.id.clone(), l.name.clone(), key, curve));
            }
        }
        for (lid, lname, key, curve) in pending {
            self.ensure_anim_blend_node(&lid, &lname, &key, true);
            let cid = Self::anim_layer_curve_id(&lid, &key);
            if let Some(n) = self.deps.node_mut(&cid) {
                n.curve = Some(curve);
            }
        }
    }

    pub fn anim_layer_blend_id(layer: &str, key: &str) -> String {
        format!("ab|{layer}|{key}")
    }

    pub fn anim_layer_curve_id(layer: &str, key: &str) -> String {
        format!("ac|{layer}|{key}")
    }

    fn channel_parts(key: &str) -> (&str, &str) {
        key.rsplit_once('.').unwrap_or(("", key))
    }

    fn channel_node_base(&self, layer: &str, key: &str) -> String {
        let (node, attr) = Self::channel_parts(key);
        let mut object = self.name_of(node);
        if object.is_empty() {
            object = node.to_string();
        }
        let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>();
        format!("{}_{}_{}", clean(&object), clean(attr), clean(layer))
    }

    fn rename_dep_node(&mut self, id: &str, base: &str) -> String {
        if let Some(n) = self.deps.node(id) {
            self.names.remove(&n.name);
        }
        let name = self.unique_name(base);
        if let Some(n) = self.deps.node_mut(id) {
            n.name = name.clone();
        }
        name
    }

    /// Create or refresh the blend node for one layered channel. A curve node is created only
    /// when `with_curve` is set or one already exists, like Maya.
    fn ensure_anim_blend_node(&mut self, layer: &str, layer_name: &str, key: &str, with_curve: bool) {
        let bid = Self::anim_layer_blend_id(layer, key);
        let cid = Self::anim_layer_curve_id(layer, key);
        let (node, attr) = Self::channel_parts(key);
        let base = self.channel_node_base(layer_name, key);
        let curve_base = format!("{base}_inputB");
        if self.deps.node(&bid).is_none() {
            let mut n = crate::graph::DepNode::new(&bid, &base, "animBlendNode");
            n.add_attr("inputA", crate::graph::AttrSpec::float(0.0));
            n.add_attr("inputB", crate::graph::AttrSpec::float(0.0));
            n.add_attr("weight", crate::graph::AttrSpec::float(1.0).range(0.0, 1.0));
            n.add_attr("output", crate::graph::AttrSpec::float(0.0).read_only());
            self.deps.insert(n);
            self.names.insert(base.clone());
        }
        if with_curve && self.deps.node(&cid).is_none() {
            let mut n = crate::graph::DepNode::new(&cid, &curve_base, "animCurve");
            n.add_attr("output", crate::graph::AttrSpec::float(0.0).read_only());
            n.add_attr("keyCount", crate::graph::AttrSpec::int(0).read_only());
            n.curve = Some(AnimCurve::new(node, attr));
            self.deps.insert(n);
            self.names.insert(curve_base.clone());
        }
        // Keep node names in sync with the object, attribute and layer names.
        let blend_name = self.rename_dep_node(&bid, &base);
        if self.deps.node(&cid).is_some() {
            self.rename_dep_node(&cid, &format!("{blend_name}_inputB"));
            if let Some(n) = self.deps.node_mut(&cid) {
                if n.curve.is_none() {
                    n.curve = Some(AnimCurve::new(node, attr));
                }
                let count = n.curve.as_ref().map(|c| c.keys.len()).unwrap_or(0) as i64;
                n.attrs.insert("keyCount".into(), serde_json::json!(count));
            }
        }
    }

    /// Make sure one layered channel owns its curve node and attribute membership.
    pub fn ensure_anim_layer_curve(&mut self, layer: &str, key: &str) -> bool {
        self.migrate_legacy_layer_curves();
        let Some((name, attrs)) = self.anim_layers.iter().find(|l| l.id == layer).map(|l| (l.name.clone(), l.attrs.clone())) else { return false };
        let _ = attrs;
        self.ensure_anim_blend_node(layer, &name, key, true);
        if let Some(l) = self.anim_layers.iter_mut().find(|l| l.id == layer) {
            if !l.attrs.contains(&key.to_string()) {
                l.attrs.push(key.to_string());
                l.attrs.sort();
                l.attrs.dedup();
            }
        }
        self.deps.node(&Self::anim_layer_curve_id(layer, key)).is_some()
    }

    /// Curve data for one layered channel.
    pub fn anim_layer_curve(&self, layer: &str, key: &str) -> Option<&AnimCurve> {
        if let Some(c) = self.deps.node(&Self::anim_layer_curve_id(layer, key)).and_then(|n| n.curve.as_ref()) {
            return Some(c);
        }
        // Compatibility until `ensure_anim_layer_nodes` migrates the old layer storage.
        self.anim_layers.iter().find(|l| l.id == layer).and_then(|l| l.curves.get(key))
    }

    pub fn anim_layer_curve_mut(&mut self, layer: &str, key: &str) -> Option<&mut AnimCurve> {
        let id = Self::anim_layer_curve_id(layer, key);
        self.deps.node_mut(&id)?.curve.as_mut()
    }

    /// Borrow every curve of a layer in attribute order.
    fn anim_layer_curve_values<'a>(&'a self, l: &'a AnimLayer) -> impl Iterator<Item = &'a AnimCurve> + 'a {
        l.attrs.iter().filter_map(move |k| self.anim_layer_curve(&l.id, k))
    }

    /// All curve entries of a layer, keyed by "node.attr".
    pub fn anim_layer_curves(&self, layer: &str) -> IndexMap<String, AnimCurve> {
        let mut out = IndexMap::new();
        let Some(l) = self.anim_layers.iter().find(|l| l.id == layer) else { return out };
        for key in &l.attrs {
            if let Some(c) = self.anim_layer_curve(layer, key) {
                out.insert(key.clone(), c.clone());
            }
        }
        out
    }

    /// Delete the blend/curve nodes belonging to one layer.
    pub fn remove_anim_layer_nodes(&mut self, layer: &str) {
        let prefix = format!("ab|{layer}|");
        let curve_prefix = format!("ac|{layer}|");
        let ids: Vec<String> = self.deps.nodes.values().filter(|n| n.id.starts_with(&prefix) || n.id.starts_with(&curve_prefix)).map(|n| n.id.clone()).collect();
        for id in ids {
            self.remove_dep_node(&id);
        }
    }

    /// Create or refresh the `animLayer` node for one layer.
    pub fn ensure_anim_layer_node(&mut self, id: &str, name: &str) {
        fn add_specs(n: &mut crate::graph::DepNode) {
            n.add_attr("weight", crate::graph::AttrSpec::float(1.0).range(0.0, 1.0));
            n.add_attr("mute", crate::graph::AttrSpec::boolean(false));
            n.add_attr("solo", crate::graph::AttrSpec::boolean(false));
            n.add_attr("lock", crate::graph::AttrSpec::boolean(false));
            n.add_attr("override", crate::graph::AttrSpec::boolean(false));
            n.add_attr("passthrough", crate::graph::AttrSpec::boolean(true));
            n.add_attr("rotationAccumulationMode", crate::graph::AttrSpec::enumeration(&["By Component", "By Layer"], "By Component"));
            n.add_attr("scaleAccumulationMode", crate::graph::AttrSpec::enumeration(&["Multiply", "Additive"], "Multiply"));
            n.add_attr("ghost", crate::graph::AttrSpec::boolean(false));
            n.add_attr("ghostColor", crate::graph::AttrSpec::vector([0.85, 0.10, 0.10]));
        }
        if let Some(n) = self.deps.node_mut(id) {
            if n.name != name {
                self.names.remove(&n.name);
                n.name = name.to_string();
            }
            self.names.insert(name.to_string());
            add_specs(n);
        } else {
            let mut n = crate::graph::DepNode::new(id, name, "animLayer");
            add_specs(&mut n);
            self.deps.insert(n);
            self.names.insert(name.to_string());
        }
    }

    /// Layer weight from its node (the scalar part; animated weights use `anim_layer_weight_at`).
    pub fn anim_layer_weight(&self, id: &str) -> f64 {
        self.deps.get(&crate::graph::PlugRef::new(id, "weight")).and_then(|v| v.as_f64()).unwrap_or(1.0)
    }

    pub fn anim_layer_weight_at(&self, l: &AnimLayer, t: f64) -> f64 {
        if l.weight_curve.keys.is_empty() {
            self.anim_layer_weight(&l.id)
        } else {
            l.weight_curve.eval(t)
        }
    }

    pub fn set_anim_layer_weight(&mut self, id: &str, weight: f64) -> bool {
        self.ensure_anim_layer_nodes();
        let ok = self.deps.set(&crate::graph::PlugRef::new(id, "weight"), serde_json::json!(weight));
        if ok {
            self.touch();
        }
        ok
    }

    pub fn find_dep(&self, name: &str) -> Option<&crate::graph::DepNode> {
        self.deps.find(name)
    }

    pub fn resolve(&self, name: &str) -> Option<NodeRef> {
        if let Some(id) = self.find(name) {
            return Some(NodeRef::Dag(id));
        }
        for n in self.nodes.values() {
            if let Some(m) = n.mesh() {
                if let Some(i) = m.history.iter().position(|hid| self.deps.node(hid).is_some_and(|h| h.name == name)) {
                    return Some(NodeRef::History(n.id.clone(), i));
                }
            }
        }
        self.deps.find(name).and_then(|n| self.shading_ref(&n.id))
    }

    pub fn ref_name(&self, r: &NodeRef) -> String {
        match r {
            NodeRef::Dag(id) => self.nodes.get(id).map(|n| n.name.clone()).unwrap_or_default(),
            NodeRef::History(id, i) => self.history_node(id, *i).map(|h| h.name).unwrap_or_default(),
            NodeRef::Material(id) | NodeRef::Texture(id) | NodeRef::Place2D(id) | NodeRef::Dependency(id) => self.deps.node(id).map(|n| n.name.clone()).unwrap_or_default(),
        }
    }

    pub fn name_of(&self, id: &str) -> String {
        self.nodes.get(id).map(|n| n.name.clone()).unwrap_or_default()
    }

    pub fn children(&self, id: Option<&str>) -> &[Id] {
        match id {
            None => &self.roots,
            Some(i) => self.nodes.get(i).map(|n| n.children.as_slice()).unwrap_or(&[]),
        }
    }

    /// Node and all of its descendants (depth first).
    pub fn subtree(&self, id: &str) -> Vec<Id> {
        let mut out = vec![];
        let mut st = vec![id.to_string()];
        while let Some(i) = st.pop() {
            if let Some(n) = self.nodes.get(&i) {
                out.push(i.clone());
                for c in n.children.iter().rev() {
                    st.push(c.clone());
                }
            }
        }
        out
    }

    /// All DAG nodes in hierarchy order (startup cameras optional).
    pub fn all_dag(&self, include_startup: bool) -> Vec<Id> {
        let mut out = vec![];
        for r in &self.roots {
            for id in self.subtree(r) {
                if include_startup || !self.nodes[&id].startup() {
                    out.push(id);
                }
            }
        }
        out
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        let mut c = self.nodes.get(b).and_then(|n| n.parent.clone());
        while let Some(p) = c {
            if p == a {
                return true;
            }
            c = self.nodes.get(&p).and_then(|n| n.parent.clone());
        }
        false
    }

    pub fn full_path(&self, id: &str) -> String {
        let mut parts = vec![];
        let mut c = Some(id.to_string());
        while let Some(i) = c {
            let Some(n) = self.nodes.get(&i) else { break };
            parts.push(n.name.clone());
            c = n.parent.clone();
        }
        parts.reverse();
        format!("|{}", parts.join("|"))
    }

    // ------------------------------------------------------------ transforms
    pub fn local_matrix(&self, id: &str) -> DMat4 {
        self.nodes.get(id).map(|n| n.xf.matrix()).unwrap_or(DMat4::IDENTITY)
    }
    pub fn world_matrix(&self, id: &str) -> DMat4 {
        let mut m = DMat4::IDENTITY;
        let mut c = Some(id.to_string());
        while let Some(i) = c {
            let Some(n) = self.nodes.get(&i) else { break };
            m = n.xf.matrix() * m;
            // Offset parent matrix sits between the parent and the local transform.
            if n.xf.opm != identity4() {
                m = n.xf.opm_matrix() * m;
            }
            // Inherits Transform off: the node keeps its own local matrix but ignores its parent.
            if !n.xf.it {
                break;
            }
            c = n.parent.clone();
        }
        m
    }
    /// The matrix the node's local transform is applied under: parent world * offset parent.
    pub fn parent_world(&self, id: &str) -> DMat4 {
        let Some(n) = self.nodes.get(id) else { return DMat4::IDENTITY };
        let opm = n.xf.opm_matrix();
        if !n.xf.it {
            return opm;
        }
        match &n.parent {
            Some(p) => self.world_matrix(p) * opm,
            None => opm,
        }
    }

    /// Set channels so the local matrix equals m (keeps pivots, shear and rotate axis).
    pub fn set_local_matrix(&mut self, id: &str, m: DMat4) {
        let Some(n) = self.nodes.get_mut(id) else { return };
        let basis = DMat3::from_cols(m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate());
        let pos = m.w_axis.truncate();
        let jo = n.xf.jo_quat();
        let ra = quat_from_euler_deg(n.xf.ra, "xyz");
        let (s, sh, q) = decompose_basis(basis, jo, ra).unwrap_or_else(|| {
            let (s, q, _) = m.to_scale_rotation_translation();
            (s, n.xf.sh, ra.inverse() * jo.inverse() * q * ra)
        });
        // joints and rotate axis keep their orient; only the remaining rotation goes into rotate
        let e = euler_deg_from_quat_near(q, &n.xf.ro, n.xf.r);
        let xf = &mut n.xf;
        xf.r = e.map(|a| round_to(a, 6));
        for i in 0..3 {
            xf.r[i] = clamp_limit(xf.r[i], &xf.lim.rotate, i);
        }
        xf.s = [s.x, s.y, s.z].map(|x| round_to(x, 6));
        for i in 0..3 {
            xf.s[i] = clamp_limit(xf.s[i], &xf.lim.scale, i);
        }
        xf.sh = sh.map(|x| round_to(x, 6));
        let rp = xf.basis().transform_point3(v3(xf.p));
        xf.t = [0, 1, 2].map(|i| round_to(pos[i] - xf.pt[i] - xf.p[i] + rp[i], 6));
        for i in 0..3 {
            xf.t[i] = clamp_limit(xf.t[i], &xf.lim.translate, i);
        }
        self.touch();
        let id = id.to_string();
        self.refresh_dependents(&id);
    }
    pub fn set_world_matrix(&mut self, id: &str, wm: DMat4) {
        let pinv = self.parent_world(id).inverse();
        self.set_local_matrix(id, pinv * wm);
    }
    pub fn world_pivot(&self, id: &str) -> DVec3 {
        let Some(n) = self.nodes.get(id) else { return DVec3::ZERO };
        let p = v3(n.xf.t) + v3(n.xf.pt) + v3(n.xf.p);
        self.parent_world(id).transform_point3(p)
    }
    pub fn set_pivot_local(&mut self, id: &str, np: [f64; 3]) {
        let Some(n) = self.nodes.get_mut(id) else { return };
        let m = n.xf.matrix();
        let rs = DMat4::from_cols(m.x_axis, m.y_axis, m.z_axis, DVec4::W);
        let d = v3(n.xf.p) - v3(np);
        let rsd = rs.transform_point3(d);
        n.xf.pt = [0, 1, 2].map(|i| n.xf.pt[i] + d[i] - rsd[i]);
        n.xf.p = np;
        self.touch();
    }
    pub fn set_pivot_world(&mut self, id: &str, wp: DVec3) {
        let lp = self.world_matrix(id).inverse().transform_point3(wp);
        self.set_pivot_local(id, lp.to_array());
    }

    /// Object-space bounding box of the shape.
    pub fn local_bbox(&self, id: &str) -> Aabb {
        let Some(n) = self.nodes.get(id) else { return Aabb::EMPTY };
        match &n.shape {
            Shape::Mesh(m) => m.mesh.bbox(),
            Shape::Curve(c) => {
                let mut b = Aabb::EMPTY;
                for p in c.sample(4) {
                    b.add(p);
                }
                b
            }
            Shape::Group => Aabb::EMPTY,
            _ => Aabb { min: DVec3::splat(-0.5), max: DVec3::splat(0.5) },
        }
    }
    /// World bounding box of the node and its descendants.
    pub fn world_bbox(&self, id: &str) -> Aabb {
        let mut b = Aabb::EMPTY;
        for i in self.subtree(id) {
            b.union(&self.local_bbox(&i).transformed(&self.world_matrix(&i)));
        }
        b
    }

    pub fn center_pivot(&mut self, id: &str) {
        let b = self.world_bbox(id);
        if !b.is_empty() {
            self.set_pivot_world(id, b.center());
        }
    }

    /// Bake transforms into mesh vertices / curve CVs and reset channels.
    pub fn freeze(&mut self, id: &str) {
        let m = self.local_matrix(id);
        let Some(n) = self.nodes.get_mut(id) else { return };
        match &mut n.shape {
            Shape::Mesh(ms) => {
                let pm = ms.mesh.transformed(&m);
                self.bake_mesh(id, pm);
            }
            Shape::Curve(c) => *c = c.transformed(&m),
            _ => {}
        }
        // children keep their world position
        let kids = self.nodes[id].children.clone();
        for k in &kids {
            let lm = self.local_matrix(k);
            self.set_local_matrix(k, m * lm);
        }
        let n = self.nodes.get_mut(id).unwrap();
        n.xf = Xform { ro: n.xf.ro.clone(), ..Default::default() };
        n.rev += 1;
        self.touch();
    }

    pub fn reset_transforms(&mut self, id: &str) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.xf.t = [0.0; 3];
            n.xf.r = [0.0; 3];
            n.xf.s = [1.0; 3];
        }
        self.touch();
    }

    // ------------------------------------------------------------ hierarchy edits
    fn detach(&mut self, id: &str) {
        let parent = self.nodes.get(id).and_then(|n| n.parent.clone());
        match parent {
            Some(p) => {
                if let Some(pn) = self.nodes.get_mut(&p) {
                    pn.children.retain(|c| c != id);
                }
            }
            None => self.roots.retain(|c| c != id),
        }
    }

    /// Re-parent (None = world), keeping the world transform. `index` inserts at a position.
    pub fn reparent(&mut self, id: &str, parent: Option<&str>, index: Option<usize>) -> bool {
        if let Some(p) = parent {
            if p == id || self.is_ancestor(id, p) || !self.nodes.contains_key(p) {
                return false;
            }
        }
        let wm = self.world_matrix(id);
        self.detach(id);
        let list = match parent {
            Some(p) => &mut self.nodes.get_mut(p).unwrap().children,
            None => &mut self.roots,
        };
        let at = index.unwrap_or(list.len()).min(list.len());
        list.insert(at, id.to_string());
        self.nodes.get_mut(id).unwrap().parent = parent.map(String::from);
        self.set_world_matrix(id, wm);
        self.touch();
        true
    }

    pub fn delete_node(&mut self, id: &str) {
        if !self.nodes.contains_key(id) {
            return;
        }
        for i in self.subtree(id) {
            if i != id && !self.nodes.contains_key(&i) {
                continue;
            }
        }
        let ids = self.subtree(id);
        self.detach(id);
        for i in &ids {
            if let Some(n) = self.nodes.shift_remove(i) {
                self.release_name(&n.name);
                if let Some(s) = &n.shape_name {
                    self.release_name(s);
                }
                if let Shape::Mesh(m) = &n.shape {
                    for hid in &m.history {
                        if let Some(h) = self.deps.remove(hid) {
                            self.names.remove(&h.name);
                        }
                    }
                }
            }
            self.remove_anim_node(i);
            self.comp.remove(i);
            self.conns.retain(|c| c.src != *i && c.dst != *i);
            self.constraints.retain(|c| c.driven != *i && !c.targets.contains(i));
        }
        self.sel.retain(|s| !ids.contains(s));
        self.hilite.retain(|h| !ids.contains(h));
        for n in self.nodes.values_mut() {
            n.published.retain(|_, p| !(matches!(p.kind.as_str(), "dag" | "history") && ids.contains(&p.node)));
        }
        // booleans etc. that used a deleted mesh keep their current result
        let users: Vec<Id> = self
            .nodes
            .values()
            .filter(|n| n.mesh().is_some_and(|m| m.history.iter().any(|hid| self.deps.node(hid).is_some_and(|h| h.attrs.values().any(|v| v.as_str().is_some_and(|s| ids.iter().any(|d| d == s)) || v.as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|s| ids.iter().any(|d| d == s)))))))))
            .map(|n| n.id.clone())
            .collect();
        for u in users {
            let pm = self.nodes[&u].mesh().unwrap().mesh.clone();
            self.bake_mesh(&u, pm);
        }
        self.touch();
    }

    pub fn group(&mut self, ids: &[Id], name: &str) -> Id {
        let parent = ids.first().and_then(|i| self.nodes.get(i)).and_then(|n| n.parent.clone());
        let g = self.create_group(name, parent.as_deref());
        for i in ids {
            self.reparent(i, Some(&g), None);
        }
        g
    }

    pub fn ungroup(&mut self, g: &str) {
        let parent = self.nodes.get(g).and_then(|n| n.parent.clone());
        let kids = self.nodes.get(g).map(|n| n.children.clone()).unwrap_or_default();
        for k in &kids {
            self.reparent(k, parent.as_deref(), None);
        }
        self.delete_node(g);
    }

    // ------------------------------------------------------------ history
    /// Dependency node at one position in a mesh's construction history.
    pub fn history_node(&self, mesh_id: &str, idx: usize) -> Option<HistoryNode> {
        let hid = self.nodes.get(mesh_id)?.mesh()?.history.get(idx)?;
        let n = self.deps.node(hid)?;
        Some(HistoryNode { id: n.id.clone(), name: n.name.clone(), ty: n.kind.clone(), params: history_params(n), out: None, error: n.error.clone() })
    }

    pub fn history_view(&self, mesh_id: &str) -> Vec<HistoryNode> {
        let len = self.nodes.get(mesh_id).and_then(|n| n.mesh()).map(|m| m.history.len()).unwrap_or(0);
        (0..len).filter_map(|i| self.history_node(mesh_id, i)).collect()
    }

    pub fn history_meta(&self, mesh_id: &str, idx: usize) -> Option<HistoryMeta> {
        let hid = self.nodes.get(mesh_id)?.mesh()?.history.get(idx)?;
        let n = self.deps.node(hid)?;
        Some(HistoryMeta { id: n.id.clone(), name: n.name.clone(), ty: n.kind.clone() })
    }

    pub fn history_metas(&self, mesh_id: &str) -> Vec<HistoryMeta> {
        let len = self.nodes.get(mesh_id).and_then(|n| n.mesh()).map(|m| m.history.len()).unwrap_or(0);
        (0..len).filter_map(|i| self.history_meta(mesh_id, i)).collect()
    }

    /// Cached output of one history node (not saved with the scene).
    pub fn history_out(&self, mesh_id: &str, idx: usize) -> Option<PolyMesh> {
        let hid = self.nodes.get(mesh_id)?.mesh()?.history.get(idx)?;
        self.deps.node(hid)?.mesh_out.clone()
    }

    pub fn history_error(&self, mesh_id: &str, idx: usize) -> Option<String> {
        let hid = self.nodes.get(mesh_id)?.mesh()?.history.get(idx)?;
        self.deps.node(hid)?.error.clone()
    }

    pub fn history_id(&self, mesh_id: &str, idx: usize) -> Option<Id> {
        self.nodes.get(mesh_id)?.mesh()?.history.get(idx).cloned()
    }

    pub fn history_find(&self, mesh_id: &str, ty: &str) -> Option<usize> {
        let m = self.nodes.get(mesh_id)?.mesh()?;
        m.history.iter().position(|id| self.deps.node(id).is_some_and(|n| n.kind == ty))
    }

    pub fn clear_history(&mut self, mesh_id: &str) {
        let ids = self.nodes.get(mesh_id).and_then(|n| n.mesh()).map(|m| m.history.clone()).unwrap_or_default();
        for hid in ids {
            if let Some(n) = self.deps.remove(&hid) {
                self.names.remove(&n.name);
            }
        }
        if let Some(m) = self.nodes.get_mut(mesh_id).and_then(|n| n.mesh_mut()) {
            m.history.clear();
        }
    }

    pub fn remove_history_node(&mut self, mesh_id: &str, hid: &str) {
        if let Some(m) = self.nodes.get_mut(mesh_id).and_then(|n| n.mesh_mut()) {
            m.history.retain(|id| id != hid);
        }
        if let Some(n) = self.deps.remove(hid) {
            self.names.remove(&n.name);
        }
        self.touch();
    }

    pub fn add_history(&mut self, id: &str, ty: &str, params: Params) -> OpOut {
        if self.nodes.get(id).and_then(|n| n.mesh()).is_none() {
            return OpOut::default();
        }
        let (base, hidden, _) = ops::info(ty).unwrap_or((ty, false, &[]));
        let hid = self.new_id("h");
        let name = if hidden { format!("{base}#{hid}") } else { self.unique_name(&format!("{base}1")) };
        let mut node = crate::graph::DepNode::new(&hid, &name, ty);
        for (k, v) in &params {
            node.add_attr(k, crate::graph::AttrSpec::any(v.clone()));
        }
        self.deps.insert(node);
        let last = if let Some(m) = self.nodes.get_mut(id).and_then(|n| n.mesh_mut()) {
            m.history.push(hid);
            m.history.len() - 1
        } else {
            return OpOut::default();
        };
        self.evaluate(id, last)
    }

    /// Re-run history from entry `from` (falls back to the start when caches are missing).
    pub fn evaluate(&mut self, id: &str, from: usize) -> OpOut {
        let Some(ms) = self.nodes.get(id).and_then(|n| n.mesh()) else { return OpOut::default() };
        let history = ms.history.clone();
        let mut start = from.min(history.len());
        let mut pm = if start > 0 {
            match self.deps.node(&history[start - 1]).and_then(|n| n.mesh_out.clone()) {
                Some(m) => m,
                None => {
                    start = 0;
                    PolyMesh::default()
                }
            }
        } else {
            PolyMesh::default()
        };
        let mut last = OpOut::default();
        for hid in history.iter().skip(start) {
            let Some(node) = self.deps.node(hid) else { continue };
            let ty = node.kind.clone();
            let params = history_params(node);
            let result = ops::apply(&ty, &pm, &params, self);
            let Some(node) = self.deps.node_mut(hid) else { continue };
            match result {
                Ok(out) => {
                    pm = out.mesh.clone();
                    node.error = None;
                    last = out;
                }
                Err(e) => {
                    node.error = Some(e);
                    last = OpOut { mesh: pm.clone(), ..Default::default() };
                }
            }
            node.mesh_out = Some(pm.clone());
        }
        let n = self.nodes.get_mut(id).unwrap();
        let ms = n.mesh_mut().unwrap();
        if !history.is_empty() {
            ms.mesh = pm;
        }
        bake_fm(ms);
        n.rev += 1;
        self.touch();
        last
    }

    pub fn set_history_param(&mut self, id: &str, idx: usize, key: &str, v: Value) {
        self.set_history_params(id, idx, &[(key.to_string(), v)]);
    }

    pub fn set_history_params(&mut self, id: &str, idx: usize, values: &[(String, Value)]) {
        let Some(hid) = self.history_id(id, idx) else { return };
        if let Some(h) = self.deps.node_mut(&hid) {
            for (key, v) in values {
                if h.spec(key).is_none() {
                    h.add_attr(key, crate::graph::AttrSpec::any(v.clone()));
                }
                h.set(key, v.clone());
            }
        }
        self.evaluate(id, idx);
        // meshes whose history reads this node (booleans, curve surfaces) follow
        self.refresh_dependents(id);
    }

    /// Re-evaluate meshes whose history references `src` (curve surfaces, booleans).
    pub fn refresh_dependents(&mut self, src: &str) {
        self.refresh_many(&[src.to_string()]);
    }

    /// Re-evaluate every mesh whose history reads one of `srcs` or their descendants
    /// (curve surfaces, booleans, deformer handles, skin joints).
    pub fn refresh_many(&mut self, srcs: &[Id]) {
        let ids: HashSet<String> = srcs.iter().flat_map(|s| self.subtree(s)).collect();
        if ids.is_empty() {
            return;
        }
        let refs = |v: &Value| v.as_str().is_some_and(|s| ids.contains(s)) || v.as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|s| ids.contains(s))));
        let users: Vec<(Id, usize)> = self
            .nodes
            .values()
            .filter_map(|n| {
                let m = n.mesh()?;
                let i = m.history.iter().position(|hid| self.deps.node(hid).is_some_and(|h| h.attrs.values().any(&refs)))?;
                Some((n.id.clone(), i))
            })
            .collect();
        for (u, i) in users {
            self.evaluate(&u, i);
        }
    }

    pub fn bake_mesh(&mut self, id: &str, pm: PolyMesh) {
        let ids = self.nodes.get(id).and_then(|n| n.mesh()).map(|m| m.history.clone()).unwrap_or_default();
        for hid in ids {
            if let Some(n) = self.deps.remove(&hid) {
                self.names.remove(&n.name);
            }
        }
        let hid = self.new_id("h");
        let mut node = crate::graph::DepNode::new(&hid, &format!("meshData#{hid}"), "meshData");
        let data = serde_json::to_value(&pm).unwrap_or(Value::Null);
        node.add_attr("data", crate::graph::AttrSpec::any(data));
        node.mesh_out = Some(pm.clone());
        self.deps.insert(node);
        if let Some(n) = self.nodes.get_mut(id) {
            if let Some(m) = n.mesh_mut() {
                m.history = vec![hid];
                m.mesh = pm;
                bake_fm(m);
            }
            n.rev += 1;
        }
        self.touch();
    }

    pub fn delete_history(&mut self, id: &str) {
        if let Some(pm) = self.nodes.get(id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) {
            self.bake_mesh(id, pm);
        }
    }

    /// Apply a modeling op to the current mesh (adds history; bakes when history is off).
    pub fn apply_op(&mut self, id: &str, ty: &str, params: Params, keep_history: bool) -> OpOut {
        if keep_history {
            return self.add_history(id, ty, params);
        }
        let Some(pm) = self.nodes.get(id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { return OpOut::default() };
        match ops::apply(ty, &pm, &params, self) {
            Ok(out) => {
                self.bake_mesh(id, out.mesh.clone());
                out
            }
            Err(e) => {
                self.warn(e);
                OpOut { mesh: pm, ..Default::default() }
            }
        }
    }

    /// Mesh in world space.
    pub fn mesh_world(&self, id: &str) -> Option<PolyMesh> {
        let m = self.nodes.get(id)?.mesh()?;
        Some(m.mesh.transformed(&self.world_matrix(id)))
    }

    // ------------------------------------------------------------ duplicate / combine / separate
    pub fn duplicate(&mut self, ids: &[Id], keep_history: bool) -> Vec<Id> {
        let mut out = vec![];
        for id in ids {
            let datas: Vec<Value> = self.subtree(id).iter().map(|i| self.serialize_node(i)).collect();
            let mut map: HashMap<String, Id> = HashMap::new();
            let parent = self.nodes.get(id).and_then(|n| n.parent.clone());
            let mut root = None;
            for mut d in datas {
                if let Some(o) = d.as_object_mut() {
                    o.remove("historyIds");
                }
                let old = d["id"].as_str().unwrap_or("").to_string();
                let par = if old == *id { parent.clone() } else { d["parent"].as_str().and_then(|p| map.get(p).cloned()) };
                if !keep_history && d["kind"] == "mesh" {
                    let mesh = d["mesh"].clone();
                    d["history"] = json!([{ "id": "x", "name": "meshData#", "type": "meshData", "params": { "data": mesh } }]);
                }
                let nid = self.deserialize_node(&d, par.as_deref(), true);
                map.insert(old, nid.clone());
                root.get_or_insert(nid);
            }
            if let Some(r) = root {
                out.push(r);
            }
        }
        out
    }

    pub fn combine(&mut self, ids: &[Id]) -> Option<Id> {
        let meshes: Vec<Id> = ids.iter().flat_map(|i| self.subtree(i)).filter(|i| self.nodes[i].mesh().is_some()).collect();
        if meshes.len() < 2 {
            return None;
        }
        let list: Vec<(PolyMesh, Option<String>)> = meshes.iter().map(|m| (self.mesh_world(m).unwrap(), Some(self.nodes[m].mesh().unwrap().material.clone()))).collect();
        let mat = list.last().unwrap().1.clone().unwrap();
        let mut pm = crate::mesh::combine_meshes(&list);
        if let Some(fm) = &mut pm.fm {
            for x in fm.iter_mut() {
                if x.as_deref() == Some(mat.as_str()) {
                    *x = None;
                }
            }
        }
        let o = self.create_mesh_from(pm, "polySurface1", Some(&mat), None);
        self.center_pivot(&o);
        for m in &meshes {
            self.delete_node(m);
        }
        for g in ids {
            if self.nodes.get(g).is_some_and(|n| n.kind == "group" && n.children.is_empty()) {
                self.delete_node(g);
            }
        }
        Some(o)
    }

    pub fn separate(&mut self, id: &str) -> Vec<Id> {
        let Some(ms) = self.nodes.get(id).and_then(|n| n.mesh()) else { return vec![] };
        let pm = ms.mesh.clone();
        let mat = ms.material.clone();
        let groups = pm.components_connected();
        if groups.len() < 2 {
            return vec![id.to_string()];
        }
        let name = self.nodes[id].name.clone();
        let parent = self.nodes[id].parent.clone();
        let wm = self.world_matrix(id);
        self.delete_node(id);
        let g = self.create_group(&name, parent.as_deref());
        let mut out = vec![];
        for fs in groups {
            let (_, piece) = crate::mesh::op_extract_faces(&pm, &fs);
            let n = self.create_mesh_from(piece, "polySurface1", Some(&mat), Some(&g));
            self.set_world_matrix(&n, wm);
            self.center_pivot(&n);
            out.push(n);
        }
        out
    }

    pub fn extract_faces(&mut self, id: &str, faces: &[usize], duplicate: bool, keep_history: bool) -> Option<Id> {
        let ms = self.nodes.get(id)?.mesh()?;
        let (_, piece) = crate::mesh::op_extract_faces(&ms.mesh, faces);
        let mat = ms.material.clone();
        let parent = self.nodes[id].parent.clone();
        let lm = self.local_matrix(id);
        let n = self.create_mesh_from(piece, "polySurface1", Some(&mat), parent.as_deref());
        self.set_local_matrix(&n, lm);
        self.center_pivot(&n);
        if !duplicate {
            let mut p = Params::new();
            p.insert("faces".into(), json!(faces));
            self.apply_op(id, "deleteComponent", p, keep_history);
        }
        Some(n)
    }

    // ------------------------------------------------------------ published attributes
    /// The internal plug a published attribute points at.
    pub fn published_ref(&self, p: &PublishedAttr) -> Option<NodeRef> {
        match p.kind.as_str() {
            "dag" => self.nodes.contains_key(&p.node).then(|| NodeRef::Dag(p.node.clone())),
            "history" => {
                let len = self.nodes.get(&p.node)?.mesh()?.history.len();
                (p.index < len).then(|| NodeRef::History(p.node.clone(), p.index))
            }
            "material" => self.is_material(&p.node).then(|| NodeRef::Material(p.node.clone())),
            "texture" => self.is_texture(&p.node).then(|| NodeRef::Texture(p.node.clone())),
            _ => None,
        }
    }

    pub fn published_target(&self, id: &str, name: &str) -> Option<(NodeRef, String)> {
        let p = self.nodes.get(id)?.published.get(name)?;
        if p.kind == "dag" && p.node == id {
            return None;
        }
        Some((self.published_ref(p)?, p.attr.clone()))
    }

    /// Resolve a published name to the internal node/attribute used by connections.
    pub fn resolve_published_plug(&self, id: &Id, attr: &str) -> (Id, String) {
        if let Some((r, a)) = self.published_target(id, attr) {
            (self.ref_id(&r), a)
        } else {
            (id.clone(), attr.to_string())
        }
    }

    /// Resolve a plug for MEL: published names first, then short attribute aliases.
    pub fn resolve_plug(&self, r: &NodeRef, attr: &str) -> (Id, String) {
        let id = self.ref_id(r);
        if let Some((target, published_attr)) = self.published_target(&id, attr) {
            (self.ref_id(&target), published_attr)
        } else {
            (id, alias(attr).to_string())
        }
    }

    /// The container a node belongs to, either through the DAG hierarchy or a published plug.
    pub fn container_of(&self, id: &str) -> Option<Id> {
        if let Some(n) = self.nodes.get(id) {
            let mut cur = n.parent.clone();
            while let Some(p) = cur {
                let parent = self.nodes.get(&p)?;
                if parent.kind == "container" {
                    return Some(p);
                }
                cur = parent.parent.clone();
            }
        }
        self.nodes.values().find(|n| n.kind == "container" && n.published.values().any(|p| p.node == id)).map(|n| n.id.clone())
    }

    /// The published name of `target.attr` on a container, if there is one.
    pub fn published_name_for(&self, container: &str, target: &NodeRef, attr: &str) -> Option<String> {
        let tid = self.ref_id(target);
        self.nodes.get(container)?.published.iter().find_map(|(name, p)| {
            (p.node == tid && p.attr == attr && self.published_ref(p).as_ref() == Some(target)).then(|| name.clone())
        })
    }

    // ------------------------------------------------------------ attributes
    /// Get an attribute of a named node (`pCube1.translateX`, `polyCube1.width`, `lambert1.color`).
    pub fn get_attr(&self, r: &NodeRef, attr: &str) -> Option<Value> {
        if let NodeRef::Dag(id) = r {
            if let Some((target, target_attr)) = self.published_target(id, attr) {
                return self.get_attr(&target, &target_attr);
            }
        }
        let attr = if matches!(r, NodeRef::Dag(_)) { alias(attr) } else { attr };
        match r {
            NodeRef::Dag(id) => {
                let n = self.nodes.get(id)?;
                if let Some((k, i)) = xf_attr(attr) {
                    return Some(json!(xf_vec(&n.xf, k)[i]));
                }
                match attr {
                    "translate" => return Some(json!(n.xf.t)),
                    "rotate" => return Some(json!(n.xf.r)),
                    "scale" => return Some(json!(n.xf.s)),
                    "rotatePivot" | "scalePivot" => return Some(json!(n.xf.p)),
                    "rotateAxis" => return Some(json!(n.xf.ra)),
                    "shear" => return Some(json!(n.xf.sh)),
                    "shearXY" => return Some(json!(n.xf.sh[0])),
                    "shearXZ" => return Some(json!(n.xf.sh[1])),
                    "shearYZ" => return Some(json!(n.xf.sh[2])),
                    "inheritsTransform" => return Some(json!(if n.xf.it { 1 } else { 0 })),
                    "offsetParentMatrix" => return Some(json!(n.xf.opm)),
                    _ if limit_attr(attr).is_some() => {
                        let (group, axis, kind) = limit_attr(attr).unwrap();
                        let l = match group {
                            1 => &n.xf.lim.rotate,
                            2 => &n.xf.lim.scale,
                            _ => &n.xf.lim.translate,
                        };
                        return Some(match kind {
                            0 => json!(l.min[axis]),
                            1 => json!(l.max[axis]),
                            _ => json!(if l.on { 1 } else { 0 }),
                        });
                    }
                    "visibility" => return Some(json!(if n.visible { 1 } else { 0 })),
                    "template" => return Some(json!(if n.template { 1 } else { 0 })),
                    "xray" => return Some(json!(if n.xray { 1 } else { 0 })),
                    "useOutlinerColor" => return Some(json!(if n.use_outliner_color { 1 } else { 0 })),
                    "outlinerColor" => return Some(json!(n.outliner_color)),
                    "rotateOrder" => return Some(json!(n.xf.ro)),
                    _ => {}
                }
                if let Some(v) = n.extra.as_object().and_then(|e| get_compound(e, attr)) {
                    return Some(v);
                }
                match &n.shape {
                    Shape::Mesh(m) => match attr {
                        "smoothLevel" => Some(json!(m.smooth_level)),
                        "softAngle" => Some(json!(m.mesh.soft_angle)),
                        "material" => Some(json!(self.material(&m.material).map(|x| x.name.clone()))),
                        _ => None,
                    },
                    Shape::Light(l) => get_compound(&l.attrs, attr),
                    Shape::Camera(c) => get_compound(serde_json::to_value(c).ok()?.as_object()?, attr),
                    Shape::Curve(c) => match attr {
                        "degree" => Some(json!(c.degree)),
                        "spans" => Some(json!(c.spans())),
                        _ => None,
                    },
                    _ => None,
                }
            }
            NodeRef::History(id, i) => {
                let hid = self.nodes.get(id)?.mesh()?.history.get(*i)?;
                let n = self.deps.node(hid)?;
                n.get(attr).or_else(|| primitives::gen_info(&n.kind).and_then(|g| g.params.iter().find(|p| p.0 == attr).map(|p| json!(p.1))))
            }
            NodeRef::Material(id) | NodeRef::Texture(id) | NodeRef::Place2D(id) => self.deps.get(&crate::graph::PlugRef::new(id, attr)),
            NodeRef::Dependency(id) => self.deps.get(&crate::graph::PlugRef::new(id, attr)),
        }
    }

    /// Set an attribute; returns false when it is locked or unknown.
    pub fn set_attr(&mut self, r: &NodeRef, attr: &str, v: Value) -> bool {
        self.set_attr_ex(r, attr, v, true)
    }

    /// `refresh` = re-evaluate meshes that depend on the node (off while batching).
    pub fn set_attr_ex(&mut self, r: &NodeRef, attr: &str, v: Value, refresh: bool) -> bool {
        if let NodeRef::Dag(id) = r {
            if let Some((target, target_attr)) = self.published_target(id, attr) {
                return self.set_attr_ex(&target, &target_attr, v, refresh);
            }
        }
        // short names (t, r, s...) are transform aliases; history params keep their own keys
        let attr = if matches!(r, NodeRef::Dag(_)) { alias(attr) } else { attr }.to_string();
        let attr = attr.as_str();
        match r {
            NodeRef::Dag(id) => {
                let Some(n) = self.nodes.get_mut(id) else { return false };
                if n.is_locked(attr) {
                    return false;
                }
                let f = v.as_f64().or_else(|| v.as_bool().map(|b| b as i32 as f64));
                if let Some((k, i)) = xf_attr(attr) {
                    let Some(mut x) = f else { return false };
                    let lim = match k {
                        "t" => Some(n.xf.lim.translate.clone()),
                        "r" => Some(n.xf.lim.rotate.clone()),
                        "s" => Some(n.xf.lim.scale.clone()),
                        _ => None,
                    };
                    if let Some(l) = lim {
                        x = clamp_limit(x, &l, i);
                    }
                    xf_vec_mut(&mut n.xf, k)[i] = x;
                } else {
                    match attr {
                        "translate" | "rotate" | "scale" => {
                            let Some(vv) = crate::params::value_vec3(&v) else { return false };
                            let mut a = vv.to_array();
                            let lim = match attr {
                                "translate" => n.xf.lim.translate.clone(),
                                "rotate" => n.xf.lim.rotate.clone(),
                                _ => n.xf.lim.scale.clone(),
                            };
                            for i in 0..3 {
                                a[i] = clamp_limit(a[i], &lim, i);
                            }
                            *xf_vec_mut(&mut n.xf, &attr[..1]) = a;
                        }
                        "rotateAxis" => {
                            let Some(vv) = crate::params::value_vec3(&v) else { return false };
                            n.xf.ra = vv.to_array();
                        }
                        "shear" => {
                            let Some(vv) = crate::params::value_vec3(&v) else { return false };
                            n.xf.sh = vv.to_array();
                        }
                        "shearXY" => n.xf.sh[0] = f.unwrap_or(0.0),
                        "shearXZ" => n.xf.sh[1] = f.unwrap_or(0.0),
                        "shearYZ" => n.xf.sh[2] = f.unwrap_or(0.0),
                        "inheritsTransform" => n.xf.it = f.unwrap_or(1.0) != 0.0,
                        "offsetParentMatrix" => {
                            if let Ok(m) = serde_json::from_value::<[[f64; 4]; 4]>(v.clone()) {
                                n.xf.opm = m;
                            } else if let Some(a) = v.as_array().filter(|a| a.len() == 16) {
                                n.xf.opm = identity4();
                                for (i, x) in a.iter().enumerate() {
                                    if let Some(x) = x.as_f64() {
                                        n.xf.opm[i % 4][i / 4] = x;
                                    }
                                }
                            } else {
                                return false;
                            }
                        }
                        _ if limit_attr(attr).is_some() => {
                            let (group, axis, kind) = limit_attr(attr).unwrap();
                            let l = match group {
                                1 => &mut n.xf.lim.rotate,
                                2 => &mut n.xf.lim.scale,
                                _ => &mut n.xf.lim.translate,
                            };
                            match kind {
                                0 => l.min[axis] = f.unwrap_or(l.min[axis]),
                                1 => l.max[axis] = f.unwrap_or(l.max[axis]),
                                _ => l.on = f.unwrap_or(1.0) != 0.0,
                            }
                        }
                        "visibility" => n.visible = f.unwrap_or(1.0) != 0.0,
                        "template" => n.template = f.unwrap_or(0.0) != 0.0,
                        "xray" => n.xray = f.unwrap_or(0.0) != 0.0,
                        "useOutlinerColor" => n.use_outliner_color = f.unwrap_or(0.0) != 0.0,
                        "outlinerColor" => {
                            let Some(c) = crate::params::value_vec3(&v) else { return false };
                            n.outliner_color = c.to_array().map(|x| x as f32);
                        }
                        "rotateOrder" => {
                            n.xf.ro = match &v {
                                Value::Number(k) => crate::math::ROTATE_ORDERS.get(k.as_u64().unwrap_or(0) as usize).copied().unwrap_or("xyz").to_string(),
                                _ => v.as_str().unwrap_or("xyz").to_string(),
                            }
                        }
                        _ if n.extra.as_object().is_some_and(|e| e.contains_key(attr) || get_compound(e, attr).is_some()) => {
                            set_compound(n.extra.as_object_mut().unwrap(), attr, v);
                            self.dynamics.frame = None;
                        }
                        _ => match &mut n.shape {
                            Shape::Mesh(m) if attr == "smoothLevel" => {
                                m.smooth_level = f.unwrap_or(0.0).clamp(0.0, 3.0) as u32;
                                n.rev += 1;
                            }
                            Shape::Mesh(m) if attr == "softAngle" => {
                                m.mesh.soft_angle = f.unwrap_or(60.0).clamp(0.0, 180.0);
                                n.rev += 1;
                            }
                            Shape::Light(l) => {
                                if !set_compound(&mut l.attrs, attr, v) {
                                    return false;
                                }
                            }
                            Shape::Camera(c) => {
                                let Ok(mut o) = serde_json::to_value(&*c) else { return false };
                                if !set_compound(o.as_object_mut().unwrap(), attr, v) {
                                    return false;
                                }
                                if let Ok(nc) = serde_json::from_value(o) {
                                    *c = nc;
                                }
                            }
                            _ => return false,
                        },
                    }
                }
                self.touch();
                if refresh {
                    let id = id.clone();
                    self.refresh_dependents(&id);
                }
                true
            }
            NodeRef::History(id, i) => {
                let (id, i) = (id.clone(), *i);
                self.set_history_param(&id, i, attr, v);
                true
            }
            NodeRef::Material(id) | NodeRef::Texture(id) | NodeRef::Place2D(id) | NodeRef::Dependency(id) => {
                let ok = self.deps.set(&crate::graph::PlugRef::new(id, attr), v);
                self.touch();
                ok
            }
        }
    }

    pub fn get_attr_f(&self, id: &str, attr: &str) -> f64 {
        self.get_attr(&NodeRef::Dag(id.into()), attr).and_then(|v| v.as_f64()).unwrap_or(0.0)
    }
    pub fn set_attr_f(&mut self, id: &str, attr: &str, v: f64) -> bool {
        self.set_attr(&NodeRef::Dag(id.into()), attr, json!(v))
    }

    /// Numeric shape attributes shown in the Channel Box SHAPES section.
    pub fn shape_channels(&self, id: &str) -> Vec<String> {
        let Some(n) = self.nodes.get(id) else { return vec![] };
        match &n.shape {
            Shape::Light(l) => l.attrs.iter().filter(|(_, v)| v.is_number() || v.is_boolean()).map(|(k, _)| k.clone()).collect(),
            Shape::Camera(_) => ["focalLength", "near", "far", "orthoWidth", "coi"].iter().map(|s| s.to_string()).collect(),
            Shape::Mesh(_) => vec!["smoothLevel".into()],
            Shape::Locator if n.kind == "emitter" || n.kind == "field" => n.extra.as_object().map(|e| e.iter().filter(|(_, v)| v.is_number()).map(|(k, _)| k.clone()).collect()).unwrap_or_default(),
            _ => vec![],
        }
    }

    /// Individual keyable attributes of a node, used for animation-layer membership.
    pub fn keyable_attrs(&self, id: &str) -> Vec<String> {
        let Some(n) = self.nodes.get(id) else { return vec![] };
        let mut list: Vec<String> = CHANNELS.iter().map(|c| c.to_string()).collect();
        if let Some(e) = n.extra.as_object() {
            list.extend(e.iter().filter(|(_, v)| v.is_number() || v.is_boolean()).map(|(k, _)| k.clone()));
        }
        list.extend(self.shape_channels(id));
        list.sort();
        list.dedup();
        list.retain(|c| n.is_keyable(alias(c)));
        list
    }

    fn anim_layer_parent(&self, l: &AnimLayer) -> Option<&AnimLayer> {
        l.parent.as_deref().and_then(|id| self.anim_layers.iter().find(|p| p.id == id))
    }

    /// Children inherit their ancestors' mutes.
    pub fn anim_layer_effective_mute(&self, l: &AnimLayer) -> bool {
        if self.anim_layer_mute(&l.id) {
            return true;
        }
        // A soloed child stays active even though Solo muted its ancestors.
        if self.anim_layers.iter().any(|x| self.anim_layer_solo(&x.id)) && self.anim_layer_soloed(l) {
            return false;
        }
        let mut p = self.anim_layer_parent(l);
        for _ in 0..self.anim_layers.len() {
            let Some(pl) = p else { break };
            if self.anim_layer_mute(&pl.id) {
                return true;
            }
            p = self.anim_layer_parent(pl);
        }
        false
    }

    /// Children's weight is multiplied by every ancestor's weight.
    pub fn anim_layer_effective_weight_at(&self, l: &AnimLayer, t: f64) -> f64 {
        let mut w = self.anim_layer_weight_at(l, t);
        let mut p = self.anim_layer_parent(l);
        for _ in 0..self.anim_layers.len() {
            let Some(pl) = p else { break };
            w *= self.anim_layer_weight_at(pl, t);
            p = self.anim_layer_parent(pl);
        }
        w
    }

    pub fn anim_layer_effective_weight(&self, l: &AnimLayer) -> f64 {
        self.anim_layer_effective_weight_at(l, self.time.current)
    }

    /// A child of a muted parent cannot be keyed.
    pub fn anim_layer_parent_muted(&self, l: &AnimLayer) -> bool {
        let mut p = self.anim_layer_parent(l);
        for _ in 0..self.anim_layers.len() {
            let Some(pl) = p else { break };
            if self.anim_layer_mute(&pl.id) {
                return true;
            }
            p = self.anim_layer_parent(pl);
        }
        false
    }

    /// A soloed parent keeps its descendants active.
    pub fn anim_layer_soloed(&self, l: &AnimLayer) -> bool {
        if self.anim_layer_solo(&l.id) {
            return true;
        }
        let mut p = self.anim_layer_parent(l);
        for _ in 0..self.anim_layers.len() {
            let Some(pl) = p else { break };
            if self.anim_layer_solo(&pl.id) {
                return true;
            }
            p = self.anim_layer_parent(pl);
        }
        false
    }

    /// Repair parent ids and rewrite the flat list as a valid tree preorder.
    pub fn normalize_anim_layer_hierarchy(&mut self) {
        let ids: std::collections::HashSet<String> = self.anim_layers.iter().map(|l| l.id.clone()).collect();
        for l in &mut self.anim_layers {
            if l.parent.as_ref().is_some_and(|p| p == &l.id || !ids.contains(p)) {
                l.parent = None;
            }
        }
        for i in 0..self.anim_layers.len() {
            let mut seen = std::collections::HashSet::new();
            let mut p = self.anim_layers[i].parent.clone();
            while let Some(id) = p {
                if !seen.insert(id.clone()) {
                    self.anim_layers[i].parent = None;
                    break;
                }
                p = self.anim_layers.iter().find(|l| l.id == id).and_then(|l| l.parent.clone());
            }
        }
        fn add(layers: &[AnimLayer], parent: Option<&str>, out: &mut Vec<AnimLayer>, seen: &mut std::collections::HashSet<String>) {
            for l in layers.iter().filter(|l| l.parent.as_deref() == parent) {
                if !seen.insert(l.id.clone()) {
                    continue;
                }
                out.push(l.clone());
                add(layers, Some(&l.id), out, seen);
            }
        }
        let mut out = vec![];
        let mut seen = std::collections::HashSet::new();
        add(&self.anim_layers, None, &mut out, &mut seen);
        for l in &self.anim_layers {
            if seen.insert(l.id.clone()) {
                out.push(l.clone());
            }
        }
        self.anim_layers = out;
    }

    /// Scenes saved before per-attribute membership only listed member objects: assign all
    /// their keyable attributes, then keep every existing layer curve.
    pub fn migrate_anim_layer_attrs(&mut self) {
        for i in 0..self.anim_layers.len() {
            if self.anim_layers[i].attrs.is_empty() {
                let mut attrs = vec![];
                for m in self.anim_layers[i].members.clone() {
                    attrs.extend(self.keyable_attrs(&m).into_iter().map(|a| Anim::key(&m, &a)));
                }
                self.anim_layers[i].attrs = attrs;
            }
            for k in self.anim_layers[i].curves.keys().cloned().collect::<Vec<_>>() {
                if !self.anim_layers[i].attrs.contains(&k) {
                    self.anim_layers[i].attrs.push(k);
                }
            }
            let nodes: Vec<String> = self.anim_layers[i].attrs.iter().filter_map(|k| k.rsplit_once('.').map(|(n, _)| n.to_string())).collect();
            for n in &nodes {
                if !self.anim_layers[i].members.contains(n) {
                    self.anim_layers[i].members.push(n.clone());
                }
            }
            self.anim_layers[i].members.retain(|m| nodes.contains(m));
        }
    }

    // ------------------------------------------------------------ animation
    pub fn set_key(&mut self, id: &str, attr: &str, t: Option<f64>, v: Option<f64>) -> bool {
        self.set_key_ex(id, attr, t, v, false)
    }

    /// Set a key; `breakdown` marks it as a breakdown key. Returns false when the layer
    /// doesn't own the attribute (or is locked).
    pub fn set_key_ex(&mut self, id: &str, attr: &str, t: Option<f64>, v: Option<f64>, breakdown: bool) -> bool {
        let attr = alias(attr);
        if self.nodes.get(id).is_some_and(|n| !n.is_keyable(attr)) {
            return false;
        }
        let t = t.unwrap_or(self.time.current);
        let mut v = v.unwrap_or_else(|| self.get_attr_f(id, attr));
        let (ti, to) = self.key_tangents.clone();
        let layer_id = self.anim_layer_current.clone().filter(|lid| self.anim_layers.iter().any(|l| &l.id == lid));
        if let Some(lid) = &layer_id {
            // A locked layer rejects new keys; a layer only receives keys for attributes
            // explicitly assigned to it.
            let Some(l) = self.anim_layers.iter().find(|l| &l.id == lid) else { return false };
            if self.anim_layer_lock(&l.id) || (!self.anim_layer_soloed(l) && self.anim_layer_parent_muted(l)) || !l.has_attr(id, attr) {
                return false;
            }
        } else if self.anim_base_lock {
            return false;
        }
        let key = Anim::key(id, attr);
        let target: Option<&mut AnimCurve> = if let Some(lid) = &layer_id {
            // Additive layers store the delta from everything below them, so keying the current
            // pose does not add the base animation a second time. Multiply-accumulated scale
            // stores the ratio to the layers below; Additive scale stores a delta.
            if let Some(l) = self.anim_layers.iter().find(|l| &l.id == lid) {
                if !self.anim_layer_override(&l.id) && attr != "visibility" {
                    let below = self.anim_value_below(&key, t, lid);
                    if attr.starts_with("scale") {
                        if self.anim_layer_scale_additive(&l.id) {
                            v -= below;
                        } else if below.abs() > 1e-12 {
                            // Multiply accumulation keys store the layer's scale ratio.
                            v /= below;
                        }
                    } else {
                        v -= below;
                    }
                }
            }
            self.ensure_anim_layer_curve(lid, &key);
            self.anim_layer_curve_mut(lid, &key)
        } else {
            self.anim.curve_mut(id, attr, true)
        };
        let Some(c) = target else { return false };
        let existed = c.find(t).is_some();
        let i = if breakdown { c.set_breakdown(t, v, &to) } else { c.set_key(t, v, &to) };
        if let Some(k) = c.keys.get_mut(i).filter(|_| !existed) {
            k.it = ti;
        }
        self.touch();
        true
    }

    /// Curves shown and edited in the timeline and graph editor: the current animation layer,
    /// or BaseAnimation when no layer is selected.
    pub fn anim_curves(&self) -> IndexMap<String, AnimCurve> {
        match self.anim_layer_current.as_ref().filter(|id| self.anim_layers.iter().any(|l| &l.id == *id)) {
            Some(id) => self.anim_layer_curves(id),
            None => self.anim.curves.clone(),
        }
    }

    /// Curve keys of the current set (layer or BaseAnimation).
    pub fn anim_curve_keys(&self) -> Vec<String> {
        match self.anim_layer_current.as_ref().filter(|id| self.anim_layers.iter().any(|l| &l.id == *id)) {
            Some(id) => self.anim_layers.iter().find(|l| &l.id == id).map(|l| l.attrs.clone()).unwrap_or_default(),
            None => self.anim.curves.keys().cloned().collect(),
        }
    }

    pub fn anim_curve_for_key(&self, key: &str) -> Option<&AnimCurve> {
        match self.anim_layer_current.as_ref().filter(|id| self.anim_layers.iter().any(|l| &l.id == *id)) {
            Some(id) => self.anim_layer_curve(id, key),
            None => self.anim.curves.get(key),
        }
    }

    pub fn anim_curve_mut_key(&mut self, key: &str) -> Option<&mut AnimCurve> {
        let current = self.anim_layer_current.clone().filter(|id| self.anim_layers.iter().any(|l| l.id == *id));
        match current {
            Some(id) => {
                self.ensure_anim_layer_curve(&id, key);
                self.anim_layer_curve_mut(&id, key)
            }
            None => {
                let (node, attr) = key.rsplit_once('.')?;
                self.anim.curve_mut(node, attr, true)
            }
        }
    }

    /// Insert or replace a curve in the current set.
    pub fn insert_anim_curve(&mut self, key: String, curve: AnimCurve) {
        let count = curve.keys.len() as i64;
        let current = self.anim_layer_current.clone().filter(|id| self.anim_layers.iter().any(|l| l.id == *id));
        match current {
            Some(id) => {
                self.ensure_anim_layer_curve(&id, &key);
                if let Some(c) = self.anim_layer_curve_mut(&id, &key) {
                    *c = curve;
                }
                if let Some(n) = self.deps.node_mut(&Self::anim_layer_curve_id(&id, &key)) {
                    n.attrs.insert("keyCount".into(), serde_json::json!(count));
                }
            }
            None => {
                self.anim.curves.insert(key, curve);
            }
        }
        self.touch();
    }

    /// Remove a curve from the current set.
    pub fn remove_anim_curve(&mut self, key: &str) -> Option<AnimCurve> {
        let current = self.anim_layer_current.clone().filter(|id| self.anim_layers.iter().any(|l| l.id == *id));
        match current {
            Some(id) => {
                let cid = Self::anim_layer_curve_id(&id, key);
                let curve = self.deps.node_mut(&cid).and_then(|n| n.curve.take());
                if let Some(l) = self.anim_layers.iter_mut().find(|l| l.id == id) {
                    l.attrs.retain(|a| a != key);
                }
                self.touch();
                curve
            }
            None => self.anim.curves.shift_remove(key),
        }
    }

    /// Visit every curve in the current set for in-place edits.
    pub fn for_each_anim_curve_mut(&mut self, mut f: impl FnMut(&mut AnimCurve)) {
        let current = self.anim_layer_current.clone().filter(|id| self.anim_layers.iter().any(|l| l.id == *id));
        match current {
            Some(id) => {
                let keys: Vec<String> = self.anim_layers.iter().find(|l| l.id == id).map(|l| l.attrs.clone()).unwrap_or_default();
                for key in keys {
                    if let Some(c) = self.anim_layer_curve_mut(&id, &key) {
                        f(c);
                    }
                }
            }
            None => {
                for c in self.anim.curves.values_mut() {
                    f(c);
                }
            }
        }
    }

    /// Remove empty current curves after key edits. Layer membership is kept; only the
    /// channel's curve/blend nodes are removed, so the next evaluation rebuilds the blend node.
    pub fn retain_anim_curves(&mut self, mut keep: impl FnMut(&AnimCurve) -> bool) {
        let current = self.anim_layer_current.clone().filter(|id| self.anim_layers.iter().any(|l| l.id == *id));
        match current {
            Some(id) => {
                let keys: Vec<String> = self.anim_layers.iter().find(|l| l.id == id).map(|l| l.attrs.clone()).unwrap_or_default();
                for key in keys {
                    let drop = self.anim_layer_curve(&id, &key).is_some_and(|c| !keep(c));
                    if drop {
                        self.remove_dep_node(&Self::anim_layer_blend_id(&id, &key));
                        self.remove_dep_node(&Self::anim_layer_curve_id(&id, &key));
                    }
                }
            }
            None => self.anim.curves.retain(|_, c| keep(c)),
        }
    }

    /// Synthetic Graph Editor key for an animation layer's weight curve.
    pub fn layer_weight_curve_key(id: &str) -> String {
        format!("/animLayerWeight/{id}")
    }

    /// A Graph Editor curve: a curve on the displayed node/layer, or a layer weight curve.
    pub fn graph_curve(&self, key: &str) -> Option<&AnimCurve> {
        if let Some(id) = key.strip_prefix("/animLayerWeight/") {
            return self.anim_layers.iter().find(|l| l.id == id).filter(|l| !l.weight_curve.keys.is_empty()).map(|l| &l.weight_curve);
        }
        self.anim_curve_for_key(key)
    }

    pub fn graph_curve_mut(&mut self, key: &str) -> Option<&mut AnimCurve> {
        if let Some(id) = key.strip_prefix("/animLayerWeight/") {
            return self.anim_layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.weight_curve);
        }
        self.anim_curve_mut_key(key)
    }

    pub fn anim_curve(&self, node: &str, attr: &str) -> Option<&AnimCurve> {
        self.anim_curve_for_key(&Anim::key(node, attr))
    }

    /// Snapshot the static channel values layers blend from. Call before the user poses the object.
    pub fn remember_anim_static(&mut self, ids: &[String]) {
        for id in ids {
            let attrs: Vec<String> = {
                let Some(n) = self.nodes.get(id) else { continue };
                let mut a: Vec<String> = CHANNELS.iter().map(|c| c.to_string()).collect();
                a.extend(self.shape_channels(id));
                if let Some(e) = n.extra.as_object() {
                    a.extend(e.iter().filter(|(_, v)| v.is_number() || v.is_boolean()).map(|(k, _)| k.clone()));
                }
                a.sort();
                a.dedup();
                a
            };
            for attr in attrs {
                let key = Anim::key(id, &attr);
                if !self.anim_static.contains_key(&key) {
                    let v = self.get_attr_f(id, &attr);
                    self.anim_static.insert(key, v);
                }
            }
        }
    }

    fn anim_static_value(&self, key: &str) -> f64 {
        self.anim_static.get(key).copied().unwrap_or_else(|| {
            let attr = key.rsplit_once('.').map(|(_, a)| a).unwrap_or("");
            // scale and visibility rest at 1; the other keyable channels rest at 0
            if attr.starts_with("scale") || attr == "visibility" { 1.0 } else { 0.0 }
        })
    }

    /// BaseAnimation value, ignoring every layer.
    fn anim_base_value(&self, key: &str, t: f64) -> f64 {
        self.anim.curves.get(key).filter(|c| !c.keys.is_empty() && !c.mute).map(|c| c.eval(t)).unwrap_or_else(|| self.anim_static_value(key))
    }

    /// Apply one layer's curve to the running value. An opaque Override layer blends with the
    /// raw base animation; Override-Passthrough blends with the layers below.
    fn anim_layer_blend(&self, l: &AnimLayer, attr: &str, v: f64, raw: f64, lv: f64, t: f64) -> f64 {
        let w = self.anim_layer_effective_weight_at(l, t);
        let override_mode = self.anim_layer_override(&l.id);
        let passthrough = self.anim_layer_passthrough(&l.id);
        if override_mode || attr == "visibility" {
            let base = if override_mode && !passthrough { raw } else { v };
            base + (lv - base) * w
        } else if attr.starts_with("scale") {
            if self.anim_layer_scale_additive(&l.id) {
                v + lv * w
            } else {
                // Maya's default additive scale accumulation is Multiply: scale^weight.
                let factor = if lv < 0.0 { -(-lv).powf(w) } else { lv.powf(w) };
                v * factor
            }
        } else {
            v + lv * w
        }
    }

    /// Final rotation for a node from BaseAnimation and the given bottom-up layer stack.
    /// Layers using By Layer rotation are combined as quaternions.
    fn anim_rotation_with_layers(&self, node: &str, t: f64, layers: &[AnimLayer]) -> [f64; 3] {
        let Some(ro) = self.nodes.get(node).map(|n| n.xf.ro.clone()) else { return [0.0; 3] };
        let base_rot = [
            self.anim_base_value(&Anim::key(node, "rotateX"), t),
            self.anim_base_value(&Anim::key(node, "rotateY"), t),
            self.anim_base_value(&Anim::key(node, "rotateZ"), t),
        ];
        let mut cur = base_rot;
        for layer in layers {
            let weight = self.anim_layer_effective_weight_at(layer, t);
            if weight <= 1e-6 {
                continue;
            }
            let override_mode = self.anim_layer_override(&layer.id);
            let passthrough = self.anim_layer_passthrough(&layer.id);
            let mut vals = [0.0, 0.0, 0.0];
            let mut any = false;
            for (i, attr) in ["rotateX", "rotateY", "rotateZ"].iter().enumerate() {
                let key = Anim::key(node, attr);
                let c = self.anim_layer_curve(&layer.id, &key);
                if let Some(c) = c.filter(|c| !c.keys.is_empty() && !c.mute) {
                    vals[i] = c.eval(t);
                    any = true;
                } else if override_mode {
                    vals[i] = cur[i];
                }
            }
            if !any {
                continue;
            }
            if self.anim_layer_rotation_by_layer(&layer.id) {
                let q_layer = quat_from_euler_deg(vals, &ro);
                let q_cur = quat_from_euler_deg(cur, &ro);
                let q_out = if override_mode {
                    let q_from = if passthrough { q_cur } else { quat_from_euler_deg(base_rot, &ro) };
                    q_from.slerp(q_layer, weight)
                } else {
                    q_cur * DQuat::IDENTITY.slerp(quat_from_euler_deg(vals, &ro), weight)
                };
                cur = euler_deg_from_quat_near(q_out, &ro, cur);
            } else {
                for (i, attr) in ["rotateX", "rotateY", "rotateZ"].iter().enumerate() {
                    if self.anim_layer_curve(&layer.id, &Anim::key(node, attr)).is_none_or(|c| c.keys.is_empty() || c.mute) {
                        continue;
                    }
                    let lv = vals[i];
                    if override_mode {
                        let base = if passthrough { cur[i] } else { base_rot[i] };
                        cur[i] = base + (lv - base) * weight;
                    } else {
                        cur[i] += lv * weight;
                    }
                }
            }
        }
        cur
    }

    /// Restore the static pose for layered channels whose layer curves are gone.
    pub fn restore_anim_static(&mut self, keys: &[String]) {
        for key in keys {
            self.restore_anim_channel(key);
        }
    }

    fn restore_anim_channel(&mut self, key: &str) -> Option<Id> {
        // an active base curve is already the base; leave it to apply_anim
        if self.anim.curves.get(key).is_some_and(|c| !c.keys.is_empty() && !c.mute) {
            return None;
        }
        let (node, attr) = key.rsplit_once('.')?;
        let v = self.anim_static_value(key);
        if self.nodes.contains_key(node) {
            self.set_attr_ex(&NodeRef::Dag(node.into()), attr, json!(v), false);
            Some(node.to_string())
        } else if let Some(r) = self.history_ref_by_id(node) {
            self.set_attr(&r, attr, json!(v));
            None
        } else {
            None
        }
    }

    /// Value an animation layer sees from BaseAnimation and the layers below it at time `t`.
    /// Computed from the curves, so repeated evaluation cannot accumulate onto the live channel.
    fn anim_value_below(&self, key: &str, t: f64, layer_id: &str) -> f64 {
        let raw = self.anim_base_value(key, t);
        let mut v = raw;
        let Some(i) = self.anim_layers.iter().position(|l| l.id == layer_id) else { return v };
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        // layers are stored top-first, so walk the ones below the current layer bottom-up
        for l in self.anim_layers[i + 1..].iter().rev() {
            if self.anim_layer_effective_mute(l) || (solo && !self.anim_layer_soloed(l)) {
                continue;
            }
            let Some(c) = self.anim_layer_curve(&l.id, key) else { continue };
            if c.keys.is_empty() || c.mute {
                continue;
            }
            v = self.anim_layer_blend(l, &c.attr, v, raw, c.eval(t), t);
        }
        v
    }

    /// Evaluated value at the output of `layer_id`, ignoring anything above it.
    fn anim_value_through(&self, key: &str, t: f64, layer_id: &str) -> f64 {
        let raw = self.anim_base_value(key, t);
        let mut v = raw;
        let Some(i) = self.anim_layers.iter().position(|l| l.id == layer_id) else { return v };
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        for l in self.anim_layers[i..].iter().rev() {
            if self.anim_layer_effective_mute(l) || (solo && !self.anim_layer_soloed(l)) {
                continue;
            }
            let Some(c) = self.anim_layer_curve(&l.id, key) else { continue };
            if c.keys.is_empty() || c.mute {
                continue;
            }
            v = self.anim_layer_blend(l, &c.attr, v, raw, c.eval(t), t);
        }
        v
    }

    /// Value below a layer with some layers ignored. Used when baking a merge.
    fn anim_value_below_skip(&self, key: &str, t: f64, layer_id: &str, skip: &HashSet<String>) -> f64 {
        let raw = self.anim_base_value(key, t);
        let mut v = raw;
        let Some(i) = self.anim_layers.iter().position(|l| l.id == layer_id) else { return v };
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        for l in self.anim_layers[i + 1..].iter().rev() {
            if skip.contains(&l.id) || self.anim_layer_effective_mute(l) || (solo && !self.anim_layer_soloed(l)) {
                continue;
            }
            let Some(c) = self.anim_layer_curve(&l.id, key) else { continue };
            if c.keys.is_empty() || c.mute {
                continue;
            }
            v = self.anim_layer_blend(l, &c.attr, v, raw, c.eval(t), t);
        }
        v
    }

    /// Evaluated value with some layers ignored. Used when flattening.
    fn anim_value_without(&self, key: &str, t: f64, skip: &HashSet<String>) -> f64 {
        let raw = self.anim_base_value(key, t);
        let mut v = raw;
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        for l in self.anim_layers.iter().rev() {
            if skip.contains(&l.id) || self.anim_layer_effective_mute(l) || (solo && !self.anim_layer_soloed(l)) {
                continue;
            }
            let Some(c) = self.anim_layer_curve(&l.id, key) else { continue };
            if c.keys.is_empty() || c.mute {
                continue;
            }
            v = self.anim_layer_blend(l, &c.attr, v, raw, c.eval(t), t);
        }
        v
    }

    fn anim_curve_key_times(&self, key: &str) -> Vec<f64> {
        let mut ts: Vec<f64> = self
            .anim
            .curves
            .get(key)
            .into_iter()
            .chain(self.anim_layers.iter().filter_map(|l| self.anim_layer_curve(&l.id, key)))
            .flat_map(|c| c.keys.iter().map(|k| k.t))
            .collect();
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        ts
    }

    fn anim_curve_times_by(&self, key: &str, sample_by: f64) -> Vec<f64> {
        let mut ts = self.anim_curve_key_times(key);
        if let (Some(lo), Some(hi)) = (ts.first().copied(), ts.last().copied()) {
            // Bake on frames (like Maya's merge) so between-key shapes survive. Very long
            // ranges are capped instead of generating a key for every frame.
            // shortcut: 2000 samples per curve, raise if long clips need more fidelity.
            let step = sample_by.max(1.0).max((hi - lo) / 2000.0);
            let mut t = if step <= 1.0 { lo.ceil() } else { lo + step };
            while t < hi {
                ts.push(t);
                t += step;
            }
            ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            ts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        }
        ts
    }

    fn anim_curve_times(&self, key: &str) -> Vec<f64> {
        self.anim_curve_times_by(key, 1.0)
    }

    fn anim_layer_subtree_ids(&self, id: &str) -> Vec<String> {
        let mut out = vec![];
        let mut stack = vec![id.to_string()];
        let mut seen = HashSet::new();
        while let Some(x) = stack.pop() {
            if !seen.insert(x.clone()) {
                continue;
            }
            out.push(x.clone());
            for l in &self.anim_layers {
                if l.parent.as_deref() == Some(x.as_str()) {
                    stack.push(l.id.clone());
                }
            }
        }
        out
    }

    pub fn anim_curve_mut(&mut self, node: &str, attr: &str, create: bool) -> Option<&mut AnimCurve> {
        let k = Anim::key(node, attr);
        if !create && self.anim_curve_for_key(&k).is_none() {
            return None;
        }
        self.anim_curve_mut_key(&k)
    }

    pub fn anim_is_keyed(&self, node: &str, attr: &str) -> bool {
        self.anim_curve(node, attr).is_some_and(|c| !c.keys.is_empty())
    }

    pub fn anim_on_key(&self, node: &str, attr: &str, t: f64) -> bool {
        self.anim_curve(node, attr).is_some_and(|c| c.find(t).is_some())
    }

    pub fn anim_key_times(&self, nodes: &[String]) -> Vec<f64> {
        let mut ts: Vec<f64> = self.anim_curves().values().filter(|c| nodes.is_empty() || nodes.contains(&c.node)).flat_map(|c| c.keys.iter().map(|k| k.t)).collect();
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        ts
    }

    pub fn anim_layer_bool(&self, id: &str, attr: &str) -> bool {
        match self.deps.get(&crate::graph::PlugRef::new(id, attr)) {
            Some(v) => v.as_bool().unwrap_or_else(|| v.as_f64().is_some_and(|x| x != 0.0)),
            // Passthrough is on for a layer whose node has not been built yet; the rest default off.
            None => attr == "passthrough",
        }
    }

    pub fn anim_layer_mute(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "mute")
    }

    pub fn anim_layer_solo(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "solo")
    }

    pub fn anim_layer_lock(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "lock")
    }

    pub fn anim_layer_ghost(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "ghost")
    }

    pub fn anim_layer_ghost_color(&self, id: &str) -> [f32; 3] {
        let v = self.deps.get(&crate::graph::PlugRef::new(id, "ghostColor")).and_then(|v| v.as_array().cloned());
        match v.as_deref() {
            Some([r, g, b, ..]) => [r.as_f64().unwrap_or(0.85) as f32, g.as_f64().unwrap_or(0.10) as f32, b.as_f64().unwrap_or(0.10) as f32],
            _ => crate::anim::ghost_red(),
        }
    }

    pub fn set_anim_layer_ghost_color(&mut self, id: &str, c: [f32; 3]) -> bool {
        self.ensure_anim_layer_nodes();
        let ok = self.deps.set(&crate::graph::PlugRef::new(id, "ghostColor"), serde_json::json!([c[0], c[1], c[2]]));
        if ok {
            self.touch();
        }
        ok
    }

    pub fn set_anim_layer_bool(&mut self, id: &str, attr: &str, on: bool) -> bool {
        self.ensure_anim_layer_nodes();
        let ok = self.deps.set(&crate::graph::PlugRef::new(id, attr), serde_json::json!(on));
        if ok {
            self.touch();
        }
        ok
    }

    /// Copy the settings plugs of one layer node to another (used when extracting/duplicating).
    pub fn copy_anim_layer_attrs(&mut self, from: &str, to: &str) {
        self.ensure_anim_layer_nodes();
        let values: Vec<(String, serde_json::Value)> = match self.deps.node(from) {
            Some(src) => src.attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            None => return,
        };
        for (attr, v) in values {
            self.deps.set(&crate::graph::PlugRef::new(to, &attr), v);
        }
    }

    pub fn anim_layer_override(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "override")
    }

    pub fn anim_layer_passthrough(&self, id: &str) -> bool {
        self.anim_layer_bool(id, "passthrough")
    }

    pub fn anim_layer_rotation_by_layer(&self, id: &str) -> bool {
        self.deps.get(&crate::graph::PlugRef::new(id, "rotationAccumulationMode")).and_then(|v| v.as_str().map(|s| s == "By Layer")).unwrap_or(false)
    }

    pub fn anim_layer_scale_additive(&self, id: &str) -> bool {
        self.deps.get(&crate::graph::PlugRef::new(id, "scaleAccumulationMode")).and_then(|v| v.as_str().map(|s| s == "Additive")).unwrap_or(false)
    }

    /// Change Additive/Override. Maya reinterprets the existing curves: additive offsets
    /// become absolute override values (or the other way round), so set the mode before
    /// adding keys. Passthrough is an independent toggle and is not changed here.
    pub fn set_anim_layer_override(&mut self, id: &str, override_mode: bool) {
        self.set_anim_layer_bool(id, "override", override_mode);
    }

    pub fn set_anim_layer_passthrough(&mut self, id: &str, passthrough: bool) {
        self.set_anim_layer_bool(id, "passthrough", passthrough);
    }

    pub fn set_anim_layer_rotation_by_layer(&mut self, id: &str, by_layer: bool) {
        self.ensure_anim_layer_nodes();
        let value = serde_json::json!(if by_layer { "By Layer" } else { "By Component" });
        if self.deps.set(&crate::graph::PlugRef::new(id, "rotationAccumulationMode"), value) {
            self.touch();
        }
    }

    pub fn set_anim_layer_scale_additive(&mut self, id: &str, additive: bool) {
        self.ensure_anim_layer_nodes();
        let value = serde_json::json!(if additive { "Additive" } else { "Multiply" });
        if self.deps.set(&crate::graph::PlugRef::new(id, "scaleAccumulationMode"), value) {
            self.touch();
        }
    }

    /// Merge `sources` (and their child layers) into `target`, baking the result onto the
    /// target's curves. Only the part of the stack up to the target is preserved, so layers
    /// above the target keep the same input. Returns false for soloed stacks, a muted target
    /// parent, zero target weight, or a target inside a source subtree.
    pub fn merge_anim_layers(&mut self, target: &str, sources: &[String]) -> bool {
        self.merge_anim_layers_with(target, sources, MergeLayerOptions::default())
    }

    pub fn merge_anim_layers_with(&mut self, target: &str, sources: &[String], opts: MergeLayerOptions) -> bool {
        let Some(target_i) = self.anim_layers.iter().position(|l| l.id == target) else { return false };
        if sources.is_empty() || sources.iter().any(|s| s == target) || self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id)) {
            return false;
        }
        self.ensure_anim_layer_nodes();
        let target_layer = self.anim_layers[target_i].clone();
        if self.anim_layer_parent_muted(&target_layer) {
            return false;
        }
        let mut merged = HashSet::new();
        for s in sources {
            if opts.include_children {
                for id in self.anim_layer_subtree_ids(s) {
                    merged.insert(id);
                }
            } else {
                merged.insert(s.clone());
            }
        }
        if merged.contains(target) {
            return false;
        }
        let mut result_override = match opts.result_mode {
            MergeLayerMode::Automatic => self.anim_layer_override(target) || self.anim_layers.iter().filter(|l| merged.contains(&l.id)).any(|l| self.anim_layer_override(&l.id)),
            MergeLayerMode::Additive => false,
            MergeLayerMode::Override => true,
        };
        // A merge-down removes the block directly above the target; the next surviving layer
        // above must see the same input, so bake the output through the top of that block.
        // Layers above a topmost target instead keep seeing the target's own output.
        let above: Vec<usize> = self.anim_layers.iter().enumerate().filter(|(_, l)| merged.contains(&l.id) && l.id != target).map(|(i, _)| i).filter(|i| *i < target_i).collect();
        let absorb = if let Some(top) = above.iter().min().copied() {
            if (top..target_i).any(|i| !merged.contains(&self.anim_layers[i].id)) {
                return false; // unselected layer inside the removed block cannot be compensated
            }
            Some(self.anim_layers[top].id.clone())
        } else {
            None
        };
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        let active: Vec<AnimLayer> = self.anim_layers.iter().rev().filter(|l| !self.anim_layer_effective_mute(l) && (!solo || self.anim_layer_soloed(l))).cloned().collect();
        let by_layer_nodes: HashSet<String> = active
            .iter()
            .filter(|l| (l.id == target || merged.contains(&l.id)) && self.anim_layer_rotation_by_layer(&l.id))
            .flat_map(|l| self.anim_layer_curve_values(l).filter(|c| c.attr.starts_with("rotate")).map(|c| c.node.clone()))
            .collect();
        let result_passthrough = !by_layer_nodes.is_empty();
        if result_passthrough {
            result_override = true;
        }
        // Only bake attributes the sources actually animate. Muted curves and zero-weight
        // layers contribute nothing, so leave the target's own curves untouched for them.
        let mut keys = HashSet::new();
        for l in self.anim_layers.iter().filter(|l| merged.contains(&l.id)) {
            if self.anim_layer_effective_mute(l) {
                continue;
            }
            for k in &l.attrs {
                if let Some(c) = self.anim_layer_curve(&l.id, k) {
                    if !c.keys.is_empty() && !c.mute && !(c.attr.starts_with("rotate") && by_layer_nodes.contains(&c.node)) {
                        keys.insert(k.clone());
                    }
                }
            }
        }
        let mut baked = vec![];
        let mut cannot_bake = false;
        for key in keys {
            let key_times = self.anim_curve_key_times(&key);
            if key_times.is_empty() {
                continue;
            }
            let mut times = if opts.smart_bake { key_times.clone() } else { self.anim_curve_times_by(&key, opts.sample_by as f64) };
            if opts.smart_bake && opts.increase_fidelity {
                let boundary = absorb.as_deref().unwrap_or(target);
                let step = opts.sample_by.max(1) as f64;
                for pair in key_times.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    let wa = self.anim_value_through(&key, a, boundary);
                    let wb = self.anim_value_through(&key, b, boundary);
                    let tol = ((wb - wa).abs() * opts.fidelity_tolerance as f64 / 100.0).max(1e-9);
                    let mut t = a + step;
                    while t < b - 1e-6 {
                        let want = self.anim_value_through(&key, t, boundary);
                        let linear = wa + (wb - wa) * (t - a) / (b - a);
                        if (want - linear).abs() > tol {
                            times.push(t);
                        }
                        t += step;
                    }
                }
                times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            }
            let attr = key.rsplit_once('.').map(|(_, a)| a).unwrap_or("");
            let samples: Vec<(f64, f64)> = times
                .into_iter()
                .filter_map(|time| {
                    let weight = self.anim_layer_effective_weight_at(&target_layer, time);
                    if weight <= 1e-6 {
                        cannot_bake = true;
                        return None;
                    }
                    let want = self.anim_value_through(&key, time, absorb.as_deref().unwrap_or(target));
                    let base = self.anim_value_below_skip(&key, time, target, &merged);
                    let raw = self.anim_base_value(&key, time);
                    let stored = if result_override {
                        let b = if result_passthrough { base } else { raw };
                        b + (want - b) / weight
                    } else if attr == "visibility" {
                        // Booleans are evaluated with the override formula even on Additive layers.
                        base + (want - base) / weight
                    } else if attr.starts_with("scale") && self.anim_layer_scale_additive(&target_layer.id) {
                        (want - base) / weight
                    } else if attr.starts_with("scale") {
                        if base.abs() <= 1e-12 {
                            if want.abs() <= 1e-12 {
                                1.0
                            } else {
                                cannot_bake = true;
                                return None;
                            }
                        } else {
                            let ratio = want / base;
                            if ratio < 0.0 { -(-ratio).powf(1.0 / weight) } else { ratio.powf(1.0 / weight) }
                        }
                    } else {
                        (want - base) / weight
                    };
                    stored.is_finite().then_some((time, stored))
                })
                .collect();
            if !samples.is_empty() {
                baked.push((key, samples));
            }
        }
        if !by_layer_nodes.is_empty() {
            let boundary = absorb.as_deref().unwrap_or(target);
            let Some(boundary_i) = active.iter().position(|l| l.id == boundary) else { return false };
            let Some(target_i_active) = active.iter().position(|l| l.id == target) else { return false };
            let want_layers = &active[..=boundary_i];
            let below: Vec<AnimLayer> = active[..target_i_active].iter().filter(|l| !merged.contains(&l.id)).cloned().collect();
            for node in &by_layer_nodes {
                let Some(ro) = self.nodes.get(node).map(|n| n.xf.ro.clone()) else { continue };
                let mut times = vec![];
                for attr in ["rotateX", "rotateY", "rotateZ"] {
                    let key = Anim::key(node, attr);
                    times.extend(if opts.smart_bake { self.anim_curve_key_times(&key) } else { self.anim_curve_times_by(&key, opts.sample_by as f64) });
                }
                times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
                if times.is_empty() {
                    continue;
                }
                let mut prev = [0.0; 3];
                let mut samples: Vec<(f64, [f64; 3])> = vec![];
                for time in times {
                    let weight = self.anim_layer_effective_weight_at(&target_layer, time);
                    if weight <= 1e-6 {
                        cannot_bake = true;
                        break;
                    }
                    let want_q = quat_from_euler_deg(self.anim_rotation_with_layers(node, time, want_layers), &ro);
                    let below_q = quat_from_euler_deg(self.anim_rotation_with_layers(node, time, &below), &ro);
                    let mut rel = below_q.inverse() * want_q;
                    if rel.w < 0.0 {
                        rel = -rel;
                    }
                    let (axis, angle) = rel.to_axis_angle();
                    if angle / weight > std::f64::consts::PI + 1e-6 {
                        cannot_bake = true;
                        break;
                    }
                    let q_stored = below_q * DQuat::from_axis_angle(axis, angle / weight);
                    let e = euler_deg_from_quat_near(q_stored, &ro, prev);
                    prev = e;
                    samples.push((time, e));
                }
                if samples.is_empty() {
                    continue;
                }
                for (i, attr) in ["rotateX", "rotateY", "rotateZ"].iter().enumerate() {
                    baked.push((Anim::key(node, attr), samples.iter().map(|(t, v)| (*t, v[i])).collect()));
                }
            }
        }
        if cannot_bake {
            return false;
        }
        self.checkpoint("merge animation layers");
        if !baked.is_empty() {
            for (key, samples) in &baked {
                let (node, attr) = key.rsplit_once('.').unwrap();
                self.ensure_anim_layer_curve(target, key);
                if let Some(c) = self.anim_layer_curve_mut(target, key) {
                    c.keys.clear();
                    c.mute = false;
                    for (time, value) in samples {
                        c.set_key(*time, *value, "auto");
                    }
                }
                let _ = (node, attr);
            }
            if let Some(t) = self.anim_layers.iter_mut().find(|l| l.id == target) {
                t.attrs.sort();
                t.attrs.dedup();
                let nodes: HashSet<String> = t.attrs.iter().filter_map(|k| k.rsplit_once('.').map(|(n, _)| n.to_string())).collect();
                t.members.retain(|m| nodes.contains(m));
                for n in nodes {
                    if !t.members.contains(&n) {
                        t.members.push(n);
                    }
                }
            }
            self.set_anim_layer_override(target, result_override);
            self.set_anim_layer_passthrough(target, result_passthrough);
            self.set_anim_layer_bool(target, "mute", false);
            if !by_layer_nodes.is_empty() {
                self.set_anim_layer_rotation_by_layer(target, true);
            }
        }
        // Maya keeps an emptied source layer when Delete Baked Layers is off, and keeps a
        // merged parent with unselected children so the hierarchy is not orphaned.
        let keep_empty: HashSet<String> = merged
            .iter()
            .filter(|id| !opts.delete_layers || (!opts.include_children && self.anim_layers.iter().any(|l| l.parent.as_deref() == Some(id.as_str()))))
            .cloned()
            .collect();
        for id in &keep_empty {
            if let Some(l) = self.anim_layers.iter_mut().find(|l| &l.id == id) {
                l.attrs.clear();
                l.members.clear();
            }
            self.remove_anim_layer_nodes(id);
        }
        self.anim_layers.retain(|l| !merged.contains(&l.id) || keep_empty.contains(&l.id));
        for id in &merged {
            if !keep_empty.contains(id) {
                self.remove_anim_layer_nodes(id);
                self.remove_dep_node(id);
            }
        }
        self.normalize_anim_layer_hierarchy();
        self.anim_layer_current = Some(target.to_string());
        self.apply_anim();
        self.touch();
        true
    }

    /// Bake every layer into BaseAnimation and remove the layer stack. The evaluated animation
    /// is unchanged; muted layers contribute nothing and are discarded.
    pub fn flatten_anim_layers(&mut self) -> bool {
        if self.anim_layers.is_empty() {
            return false;
        }
        self.ensure_anim_layer_nodes();
        let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
        let active: Vec<AnimLayer> = self.anim_layers.iter().rev().filter(|l| !self.anim_layer_effective_mute(l) && (!solo || self.anim_layer_soloed(l))).cloned().collect();
        let by_layer_nodes: HashSet<String> = active
            .iter()
            .filter(|l| self.anim_layer_rotation_by_layer(&l.id))
            .flat_map(|l| self.anim_layer_curve_values(l).filter(|c| c.attr.starts_with("rotate")).map(|c| c.node.clone()))
            .collect();
        let mut keys = HashSet::new();
        for l in &self.anim_layers {
            for k in &l.attrs {
                if self.anim_layer_curve(&l.id, k).is_some_and(|c| !c.keys.is_empty()) {
                    keys.insert(k.clone());
                }
            }
        }
        let empty = HashSet::new();
        let mut baked = vec![];
        for key in keys {
            if let Some((node, attr)) = key.rsplit_once('.') {
                if attr.starts_with("rotate") && by_layer_nodes.contains(node) {
                    continue;
                }
            }
            let times = self.anim_curve_times(&key);
            if times.is_empty() {
                continue;
            }
            let samples: Vec<(f64, f64)> = times.into_iter().map(|t| (t, self.anim_value_without(&key, t, &empty))).collect();
            baked.push((key, samples));
        }
        for node in &by_layer_nodes {
            let mut times = vec![];
            for attr in ["rotateX", "rotateY", "rotateZ"] {
                times.extend(self.anim_curve_times(&Anim::key(node, attr)));
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            if times.is_empty() {
                continue;
            }
            let samples: Vec<(f64, [f64; 3])> = times.iter().map(|t| (*t, self.anim_rotation_with_layers(node, *t, &active))).collect();
            for (i, attr) in ["rotateX", "rotateY", "rotateZ"].iter().enumerate() {
                baked.push((Anim::key(node, attr), samples.iter().map(|(t, v)| (*t, v[i])).collect()));
            }
        }
        self.checkpoint("flatten animation layers");
        for (key, samples) in baked {
            let Some((node, attr)) = key.rsplit_once('.') else { continue };
            if let Some(c) = self.anim.curve_mut(node, attr, true) {
                c.keys.clear();
                c.mute = false;
                for (time, value) in samples {
                    c.set_key(time, value, "auto");
                }
            }
        }
        let old_ids: Vec<String> = self.anim_layers.iter().map(|l| l.id.clone()).collect();
        self.anim_layers.clear();
        for id in old_ids {
            self.remove_anim_layer_nodes(&id);
            self.remove_dep_node(&id);
        }
        self.anim_layer_current = None;
        self.anim_solo_saved.clear();
        self.apply_anim();
        self.touch();
        true
    }

    /// Move the curves of `nodes` from `source` into a new sibling layer placed directly above
    /// it, so the evaluated result is preserved. Returns the new layer id.
    pub fn extract_anim_layer_objects(&mut self, source: &str, nodes: &[String], name: &str) -> Option<String> {
        let source_i = self.anim_layers.iter().position(|l| l.id == source)?;
        self.migrate_legacy_layer_curves();
        let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
        let keys: Vec<String> = self.anim_layers[source_i]
            .attrs
            .iter()
            .filter(|k| self.anim_layer_curve(source, k).is_some_and(|c| node_set.contains(c.node.as_str())))
            .cloned()
            .collect();
        if keys.is_empty() {
            return None;
        }
        self.checkpoint("extract animation layer objects");
        let id = self.new_id("al");
        let src = self.anim_layers[source_i].clone();
        let mut attrs = keys.clone();
        attrs.sort();
        attrs.dedup();
        let mut members: Vec<String> = attrs.iter().filter_map(|k| k.rsplit_once('.').map(|(n, _)| n.to_string())).collect();
        members.sort();
        members.dedup();
        self.anim_layers.insert(
            source_i,
            AnimLayer {
                id: id.clone(),
                name: name.to_string(),
                weight_curve: src.weight_curve.clone(),
                members,
                attrs,
                parent: src.parent.clone(),
                curves: Default::default(),
            },
        );
        self.ensure_anim_layer_nodes();
        self.copy_anim_layer_attrs(source, &id);
        for key in &keys {
            if let Some(c) = self.anim_layer_curve(source, key).cloned() {
                self.ensure_anim_layer_curve(&id, key);
                if let Some(dst) = self.anim_layer_curve_mut(&id, key) {
                    *dst = c;
                }
            }
        }
        if let Some(s) = self.anim_layers.iter_mut().find(|l| l.id == source) {
            s.attrs.retain(|a| !keys.contains(a));
            let remaining: HashSet<String> = s.attrs.iter().filter_map(|k| k.rsplit_once('.').map(|(n, _)| n.to_string())).collect();
            s.members.retain(|m| remaining.contains(m));
        }
        self.anim_layer_current = Some(id.clone());
        self.apply_anim();
        self.touch();
        Some(id)
    }

    /// Move non-layered (BaseAnimation) curves for `nodes` to a new top Override layer.
    /// Returns the new layer id, or None when those objects have no base animation.
    pub fn extract_base_animation(&mut self, nodes: &[String], base_name: &str) -> Option<String> {
        let node_set: HashSet<&str> = nodes.iter().map(|s| s.as_str()).collect();
        let keys: Vec<String> = self.anim.curves.iter().filter(|(_, c)| node_set.contains(c.node.as_str()) && !c.keys.is_empty()).map(|(k, _)| k.clone()).collect();
        if keys.is_empty() {
            return None;
        }
        self.checkpoint("extract non-layered animation");
        let id = self.new_id("al");
        let mut name = base_name.to_string();
        let mut i = 1;
        while self.anim_layers.iter().any(|l| l.name == name) {
            name = format!("{base_name}{i}");
            i += 1;
        }
        let mut curves = IndexMap::new();
        for key in &keys {
            if let Some(c) = self.anim.curves.shift_remove(key) {
                curves.insert(key.clone(), c);
            }
        }
        let mut attrs = keys;
        attrs.sort();
        attrs.dedup();
        let mut members: Vec<String> = attrs.iter().filter_map(|k| k.rsplit_once('.').map(|(n, _)| n.to_string())).collect();
        members.sort();
        members.dedup();
        self.anim_layers.insert(
            0,
            AnimLayer {
                id: id.clone(),
                name,
                weight_curve: Default::default(),
                members,
                attrs,
                parent: None,
                curves,
            },
        );
        self.anim_layer_current = Some(id.clone());
        self.ensure_anim_layer_nodes();
        self.set_anim_layer_override(&id, true);
        self.set_anim_layer_passthrough(&id, true);
        self.apply_anim();
        self.touch();
        Some(id)
    }

    /// Remove a node's curves from the base animation and every animation layer.
    pub fn remove_anim_node(&mut self, node: &str) {
        self.anim.remove_node(node);
        let mut removed = vec![];
        for l in &mut self.anim_layers {
            let before = l.attrs.len();
            l.attrs.retain(|k| {
                let hit = k.rsplit_once('.').is_some_and(|(n, _)| n == node);
                if hit {
                    removed.push((l.id.clone(), k.clone()));
                }
                !hit
            });
            let _ = before;
        }
        for (lid, key) in removed {
            self.remove_dep_node(&Self::anim_layer_blend_id(&lid, &key));
            self.remove_dep_node(&Self::anim_layer_curve_id(&lid, &key));
        }
    }

    /// Evaluate per-channel expressions. A few passes let expressions that reference other
    /// expressed channels settle without needing a dependency graph.
    pub fn apply_expressions(&mut self) -> Vec<Id> {
        let list: Vec<(Id, IndexMap<String, String>)> = self
            .nodes
            .iter()
            .filter(|(_, n)| !n.expressions.is_empty())
            .map(|(id, n)| (id.clone(), n.expressions.clone()))
            .collect();
        let time = self.time.current;
        let mut moved = vec![];
        for (id, exprs) in list {
            let mut changed = false;
            for _ in 0..3 {
                let mut vars: HashMap<String, f64> = HashMap::new();
                for c in CHANNELS {
                    vars.insert(c.to_string(), self.get_attr_f(&id, c));
                }
                if let Some(e) = self.nodes.get(&id).and_then(|n| n.extra.as_object()) {
                    for (k, v) in e {
                        if let Some(x) = v.as_f64() {
                            vars.insert(k.clone(), x);
                        } else if let Some(b) = v.as_bool() {
                            vars.insert(k.clone(), b as i32 as f64);
                        }
                    }
                }
                for c in self.shape_channels(&id) {
                    vars.insert(c.clone(), self.get_attr_f(&id, &c));
                }
                let mut vals = vec![];
                for (attr, src) in &exprs {
                    if let Ok(v) = crate::expr::eval(src, &|name| vars.get(name).copied(), time) {
                        vals.push((attr.clone(), v));
                    }
                }
                for (attr, v) in vals {
                    if (self.get_attr_f(&id, &attr) - v).abs() > 1e-12 {
                        self.set_attr_ex(&NodeRef::Dag(id.clone()), &attr, json!(v), false);
                        changed = true;
                    }
                }
            }
            if changed {
                moved.push(id);
            }
        }
        if !moved.is_empty() {
            self.touch();
        }
        moved
    }

    /// Evaluate anim curves at the current time and push values into channels.
    pub fn apply_anim(&mut self) {
        self.apply_anim_upto(Some(0));
    }

    /// Evaluate BaseAnimation and the layers from the bottom up to and including `upto`.
    /// `None` evaluates BaseAnimation alone (used when building layer ghosts).
    pub fn apply_anim_upto(&mut self, upto: Option<usize>) {
        self.ensure_anim_layer_nodes();
        let t = self.time.current;
        let vals: Vec<(String, String, f64)> = self.anim.curves.values().filter(|c| !c.keys.is_empty() && !c.mute).map(|c| (c.node.clone(), c.attr.clone(), c.eval(t))).collect();
        let mut moved: Vec<Id> = vec![];
        for (n, a, v) in vals {
            if self.nodes.contains_key(&n) {
                let r = NodeRef::Dag(n.clone());
                self.set_attr_ex(&r, &a, json!(v), false);
                if !moved.contains(&n) {
                    moved.push(n);
                }
            } else if let Some(r) = self.history_ref_by_id(&n) {
                self.set_attr(&r, &a, json!(v));
            }
        }
        // Layers blend from the static pose, so muted or zero-weight layers return to it
        // instead of leaving the value from the previous evaluation.
        let mut driven: Vec<String> = self.anim_layers.iter().flat_map(|l| l.attrs.iter().cloned()).collect();
        driven.sort();
        driven.dedup();
        for key in &driven {
            if let Some(n) = self.restore_anim_channel(key) {
                if !moved.contains(&n) {
                    moved.push(n);
                }
            }
        }
        // Animation layers, bottom-to-top so the upper layers win. Solo silences the rest.
        if let Some(upto) = upto {
            let start = upto.min(self.anim_layers.len());
            let solo = self.anim_layers.iter().any(|l| self.anim_layer_solo(&l.id));
            let layers: Vec<AnimLayer> = self.anim_layers[start..]
                .iter()
                .rev()
                .filter(|l| !self.anim_layer_effective_mute(l) && (!solo || self.anim_layer_soloed(l)))
                .cloned()
                .collect();
            let by_layer_nodes: HashSet<String> = layers
                .iter()
                .filter(|l| self.anim_layer_rotation_by_layer(&l.id))
                .flat_map(|l| self.anim_layer_curve_values(l).filter(|c| c.attr.starts_with("rotate")).map(|c| c.node.clone()))
                .collect();
            // Snapshot the curves so the channel writes below do not hold a borrow on the graph.
            let entries: Vec<(String, String, String, f64)> = layers
                .iter()
                .flat_map(|l| {
                    self.anim_layer_curve_values(l)
                        .filter(|c| !c.keys.is_empty() && !c.mute)
                        .map(|c| (l.id.clone(), c.node.clone(), c.attr.clone(), c.eval(t)))
                        .collect::<Vec<_>>()
                })
                .collect();
            for (lid, node, attr, lv) in entries {
                if !self.nodes.contains_key(&node) && self.history_ref_by_id(&node).is_none() {
                    continue;
                }
                if attr.starts_with("rotate") && by_layer_nodes.contains(&node) {
                    continue;
                }
                let key = Anim::key(&node, &attr);
                let Some(layer) = layers.iter().find(|l| l.id == lid) else { continue };
                let base = self.anim_value_below(&key, t, &lid);
                // Boolean channels (visibility) always behave as override, as in Maya.
                let v = self.anim_layer_blend(layer, &attr, base, self.anim_base_value(&key, t), lv, t);
                self.set_layer_blend_plugs(&lid, &key, base, lv, self.anim_layer_effective_weight_at(layer, t), v);
                if self.nodes.contains_key(&node) {
                    let r = NodeRef::Dag(node.clone());
                    self.set_attr_ex(&r, &attr, json!(v), false);
                    if !moved.contains(&node) {
                        moved.push(node.clone());
                    }
                } else if let Some(r) = self.history_ref_by_id(&node) {
                    self.set_attr(&r, &attr, json!(v));
                }
            }
            for node in &by_layer_nodes {
                let cur = self.anim_rotation_with_layers(node, t, &layers);
                for (i, attr) in ["rotateX", "rotateY", "rotateZ"].iter().enumerate() {
                    let r = NodeRef::Dag(node.clone());
                    self.set_attr_ex(&r, attr, json!(cur[i]), false);
                }
                if !moved.contains(node) {
                    moved.push(node.clone());
                }
            }
        }
        moved.extend(self.eval_connections());
        moved.extend(self.apply_expressions());
        moved.extend(self.eval_constraints());
        self.refresh_many(&moved);
    }

    /// Publish the current evaluation into the channel's blend node and curve output.
    fn set_layer_blend_plugs(&mut self, layer: &str, key: &str, input_a: f64, input_b: f64, weight: f64, output: f64) {
        let cid = Self::anim_layer_curve_id(layer, key);
        let bid = Self::anim_layer_blend_id(layer, key);
        if let Some(n) = self.deps.node_mut(&cid) {
            n.attrs.insert("output".into(), json!(input_b));
            let count = n.curve.as_ref().map(|c| c.keys.len()).unwrap_or(0) as i64;
            n.attrs.insert("keyCount".into(), json!(count));
        }
        for (attr, v) in [("inputA", input_a), ("inputB", input_b), ("weight", weight)] {
            self.deps.set(&crate::graph::PlugRef::new(&bid, attr), json!(v));
        }
        if let Some(n) = self.deps.node_mut(&bid) {
            n.attrs.insert("output".into(), json!(output));
        }
    }

    /// Propagate direct connections (`dst = src`) until values settle; returns the DAG nodes
    /// that changed. Runs every frame so connected attributes follow gizmo drags and channel
    /// edits live, not only when the timeline moves.
    pub fn eval_connections(&mut self) -> Vec<Id> {
        let mut moved: Vec<Id> = vec![];
        // chains can need one pass per link; the cap also stops A -> B -> A cycles
        for _ in 0..self.conns.len().max(1) {
            let mut changed = false;
            for c in self.conns.clone() {
                let (Some(sr), Some(dr)) = (self.ref_by_id(&c.src), self.ref_by_id(&c.dst)) else { continue };
                let Some(v) = self.get_attr(&sr, &c.src_attr) else { continue };
                // numbers can round-trip as int/float JSON values; treat them as equal
                let unchanged = self.get_attr(&dr, &c.dst_attr).is_some_and(|cur| {
                    cur == v || (cur.is_number() && v.is_number() && cur.as_f64() == v.as_f64())
                });
                if unchanged {
                    continue;
                }
                if self.set_attr_ex(&dr, &c.dst_attr, v, false) {
                    changed = true;
                    if let NodeRef::Dag(id) = &dr {
                        if !moved.contains(id) {
                            moved.push(id.clone());
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        moved
    }

    // ------------------------------------------------------------ constraints
    /// Apply all constraints; returns the nodes that moved.
    pub fn eval_constraints(&mut self) -> Vec<Id> {
        let mut moved = self.eval_rig_pre();
        for c in self.constraints.clone() {
            if c.kind == "poleVector" {
                continue;
            }
            if !self.nodes.contains_key(&c.driven) || c.targets.iter().any(|t| !self.nodes.contains_key(t)) || c.targets.is_empty() {
                continue;
            }
            let cur = self.world_matrix(&c.driven);
            let (cs, cq, ct) = cur.to_scale_rotation_translation();
            let n = c.targets.len() as f64;
            let tw: Vec<DMat4> = c.targets.iter().map(|t| self.world_matrix(t)).collect();
            let avg_pos = c.targets.iter().map(|t| self.world_pivot(t)).sum::<DVec3>() / n;
            let off = c.offset.map(|o| DMat4::from_cols_array(&o)).unwrap_or(DMat4::IDENTITY);
            let new = match c.kind.as_str() {
                "point" => {
                    let pivot_off = self.world_pivot(&c.driven) - ct;
                    DMat4::from_scale_rotation_translation(cs, cq, avg_pos + off.w_axis.truncate() - pivot_off)
                }
                "orient" => {
                    let mut q = tw[0].to_scale_rotation_translation().1;
                    for (i, m) in tw.iter().enumerate().skip(1) {
                        q = q.slerp(m.to_scale_rotation_translation().1, 1.0 / (i as f64 + 1.0));
                    }
                    let oq = DQuat::from_mat4(&off);
                    DMat4::from_scale_rotation_translation(cs, q * oq, ct)
                }
                "scale" => {
                    let s = tw.iter().map(|m| m.to_scale_rotation_translation().0).sum::<DVec3>() / n;
                    DMat4::from_scale_rotation_translation(s * off.to_scale_rotation_translation().0, cq, ct)
                }
                "aim" => {
                    let aim = DVec3::from_array(c.aim).normalize_or(DVec3::X);
                    let up = DVec3::from_array(c.up).normalize_or(DVec3::Y);
                    let dir = (avg_pos - self.world_pivot(&c.driven)).normalize_or(aim);
                    // rotate the aim axis onto dir, then roll the up axis towards world up
                    let q1 = DQuat::from_rotation_arc(aim, dir);
                    let up1 = q1 * up;
                    let want = (DVec3::Y - dir * DVec3::Y.dot(dir)).normalize_or(up1);
                    let ang = up1.cross(want).dot(dir).atan2(up1.dot(want));
                    let q = DQuat::from_axis_angle(dir, ang) * q1;
                    DMat4::from_scale_rotation_translation(cs, q, ct)
                }
                _ => tw[0] * off, // parent
            };
            if (new - cur).to_cols_array().iter().any(|x| x.abs() > 1e-9) {
                let pinv = self.parent_world(&c.driven).inverse();
                let local = pinv * new;
                // quiet update: no refresh here, the caller batches it
                let Some(nd) = self.nodes.get_mut(&c.driven) else { continue };
                let (s, q, pos) = local.to_scale_rotation_translation();
                let e = euler_deg_from_quat_near(nd.xf.jo_quat().inverse() * q, &nd.xf.ro, nd.xf.r);
                let xf = &mut nd.xf;
                xf.r = e;
                xf.s = s.to_array();
                let rs = DMat4::from_scale_rotation_translation(s, q, DVec3::ZERO);
                let rp = rs.transform_point3(v3(xf.p));
                xf.t = [0, 1, 2].map(|i| pos[i] - xf.pt[i] - xf.p[i] + rp[i]);
                self.rev += 1;
                moved.push(c.driven.clone());
            }
        }
        moved.extend(self.eval_rig_post());
        moved
    }

    /// Create a constraint (`maintain_offset` keeps the driven object where it is).
    pub fn add_constraint(&mut self, kind: &str, targets: Vec<Id>, driven: &str, maintain_offset: bool) -> String {
        let name = self.unique_name(&format!("{}_{kind}Constraint1", self.name_of(driven)));
        let id = self.new_id("c");
        let dw = self.world_matrix(driven);
        let offset = if maintain_offset {
            match kind {
                "point" => {
                    let avg = targets.iter().map(|t| self.world_pivot(t)).sum::<DVec3>() / targets.len().max(1) as f64;
                    Some(DMat4::from_translation(self.world_pivot(driven) - avg).to_cols_array())
                }
                "orient" => {
                    let tq = self.world_matrix(&targets[0]).to_scale_rotation_translation().1;
                    let dq = dw.to_scale_rotation_translation().1;
                    Some(DMat4::from_quat(tq.inverse() * dq).to_cols_array())
                }
                "scale" => {
                    let ts = self.world_matrix(&targets[0]).to_scale_rotation_translation().0;
                    let ds = dw.to_scale_rotation_translation().0;
                    Some(DMat4::from_scale(ds / ts.max(DVec3::splat(1e-9))).to_cols_array())
                }
                "parent" => Some((self.world_matrix(&targets[0]).inverse() * dw).to_cols_array()),
                _ => None,
            }
        } else if kind == "parent" {
            // snap: keep the driven object's own scale
            let (s, _, _) = dw.to_scale_rotation_translation();
            Some(DMat4::from_scale(s).to_cols_array())
        } else {
            None
        };
        self.constraints.push(Constraint { id, name: name.clone(), kind: kind.into(), driven: driven.into(), targets, offset, aim: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0] });
        let moved = self.eval_constraints();
        self.refresh_many(&moved);
        self.touch();
        name
    }

    pub fn set_time(&mut self, t: f64) {
        self.time.current = t;
        self.apply_anim();
        self.step_dynamics(t);
        self.rev += 1;
    }

    pub fn ref_by_id(&self, id: &str) -> Option<NodeRef> {
        if self.nodes.contains_key(id) {
            return Some(NodeRef::Dag(id.into()));
        }
        if self.deps.nodes.contains_key(id) {
            if let Some(r) = self.history_ref_by_id(id) {
                return Some(r);
            }
            return self.shading_ref(id);
        }
        self.history_ref_by_id(id)
    }
    fn history_ref_by_id(&self, hid: &str) -> Option<NodeRef> {
        for n in self.nodes.values() {
            if let Some(i) = n.mesh().and_then(|m| m.history.iter().position(|h| h == hid)) {
                return Some(NodeRef::History(n.id.clone(), i));
            }
        }
        None
    }
    pub fn ref_id(&self, r: &NodeRef) -> Id {
        match r {
            NodeRef::Dag(i) | NodeRef::Material(i) | NodeRef::Texture(i) | NodeRef::Place2D(i) | NodeRef::Dependency(i) => i.clone(),
            NodeRef::History(n, i) => self.nodes.get(n).and_then(|n| n.mesh()).and_then(|m| m.history.get(*i)).cloned().unwrap_or_default(),
        }
    }

    // ------------------------------------------------------------ selection
    pub fn select(&mut self, ids: &[Id], mode: SelMode) {
        // Unselectable nodes never enter the selection. Templated nodes do: Maya only blocks
        // them in the view, while the Outliner and Hypergraph can still select them.
        let ids: Vec<Id> = ids.iter().filter(|i| self.nodes.get(*i).is_some_and(|n| n.selectable)).cloned().collect();
        let ids = &ids[..];
        match mode {
            SelMode::Replace => self.sel = ids.to_vec(),
            SelMode::Add => {
                for i in ids {
                    self.sel.retain(|s| s != i);
                    self.sel.push(i.clone());
                }
            }
            SelMode::Toggle => {
                for i in ids {
                    if self.sel.contains(i) {
                        self.sel.retain(|s| s != i);
                    } else {
                        self.sel.push(i.clone());
                    }
                }
            }
            SelMode::Remove => self.sel.retain(|s| !ids.contains(s)),
        }
        self.sel.retain(|s| self.nodes.contains_key(s));
        if self.anim_layer_current.is_some() {
            let ids = self.sel.clone();
            self.remember_anim_static(&ids);
        }
        self.rev += 1;
    }
    pub fn lead(&self) -> Option<&Id> {
        self.sel.last()
    }

    /// Objects whose components are being edited (the JS `App.hilite` list).
    pub fn hilite(&self) -> Vec<Id> {
        if self.comp_mode.is_none() {
            return vec![];
        }
        self.hilite.iter().filter(|i| self.nodes.get(*i).is_some_and(|n| n.mesh().is_some() || n.curve().is_some())).cloned().collect()
    }

    /// Replace the component-mode hilite with the given objects, filtering to meshes and curves
    /// as the JS `Sel.hilite` does.
    pub fn set_hilite(&mut self, ids: &[Id]) {
        self.hilite.clear();
        for id in ids {
            if self.hilite.contains(id) {
                continue;
            }
            if self.nodes.get(id).is_some_and(|n| n.mesh().is_some() || n.curve().is_some()) {
                self.hilite.push(id.clone());
            }
        }
    }

    /// Add an object to the component-mode hilite (JS `Sel.selectComponents` line 55).
    pub fn add_hilite(&mut self, id: &Id) {
        if !self.hilite.contains(id) && self.nodes.get(id).is_some_and(|n| n.mesh().is_some() || n.curve().is_some()) {
            self.hilite.push(id.clone());
        }
    }

    pub fn set_comp_mode(&mut self, m: Option<CompMode>) {
        match (self.comp_mode, m) {
            (None, Some(to)) => {
                // entering component mode: the selected meshes and curves become the hilite
                let cand: Vec<Id> = self
                    .sel
                    .iter()
                    .filter(|i| self.nodes.get(*i).is_some_and(|n| n.mesh().is_some() || n.curve().is_some()))
                    .cloned()
                    .collect();
                if !cand.is_empty() {
                    self.set_hilite(&cand);
                }
                self.hilite.retain(|i| self.nodes.contains_key(i));
                self.comp_mode = Some(to);
                self.rev += 1;
            }
            (Some(_), None) => {
                // back to object mode: objects with components selected stay selected, at the
                // end of the selection as in the JS (they become the lead)
                let keep = std::mem::take(&mut self.hilite);
                self.comp_mode = None;
                for id in keep {
                    self.sel.retain(|s| s != &id);
                    self.sel.push(id);
                }
                self.rev += 1;
            }
            _ => {
                // Maya stores vertex, edge and face selections separately; switching modes
                // restores that mode's previous set and never converts implicitly.
                self.comp_mode = m;
                self.rev += 1;
            }
        }
    }

    pub fn comp_sel(&self, id: &str) -> CompSel {
        self.comp.get(id).cloned().unwrap_or_default()
    }

    /// Clear the active component mode's selection, leaving the other modes stored.
    pub fn clear_active_comp(&mut self, id: &str) {
        let empty = if let Some(c) = self.comp.get_mut(id) {
            match self.comp_mode {
                Some(CompMode::Vertex) => c.v.clear(),
                Some(CompMode::Edge) => c.e.clear(),
                Some(CompMode::Face) => c.f.clear(),
                None => {}
            }
            c.is_empty()
        } else {
            false
        };
        if empty {
            self.comp.remove(id);
        }
    }

    /// The selection of the active component mode only.
    pub fn comp_sel_active(&self, id: &str) -> CompSel {
        let c = self.comp_sel(id);
        let mut out = CompSel::default();
        match self.comp_mode {
            Some(CompMode::Vertex) => out.v = c.v,
            Some(CompMode::Edge) => out.e = c.e,
            Some(CompMode::Face) => out.f = c.f,
            None => {}
        }
        out
    }

    /// Vertices of the active component selection. Curve CVs count as vertices
    /// (`o.inca.curve.cvs` in the JS `Sel.vertsOf`).
    pub fn comp_verts(&self, id: &str) -> Vec<u32> {
        let Some(n) = self.nodes.get(id) else { return vec![] };
        if let Some(c) = n.curve() {
            return self.comp_sel(id).v.into_iter().filter(|&v| (v as usize) < c.cvs.len()).collect();
        }
        let Some(pm) = n.mesh().map(|m| &m.mesh) else { return vec![] };
        let c = self.comp_sel(id);
        let mut s: BTreeSet<u32> = BTreeSet::new();
        // Object mode has no active components; keep the old union for tools that inspect it.
        match self.comp_mode {
            Some(CompMode::Vertex) => s.extend(c.v.iter().copied()),
            Some(CompMode::Edge) => {
                for (a, b) in &c.e {
                    s.insert(*a);
                    s.insert(*b);
                }
            }
            Some(CompMode::Face) => {
                for &f in &c.f {
                    if let Some(face) = pm.f.get(f) {
                        s.extend(face.iter().copied());
                    }
                }
            }
            None => {
                s.extend(c.v.iter().copied());
                for (a, b) in &c.e {
                    s.insert(*a);
                    s.insert(*b);
                }
                for &f in &c.f {
                    if let Some(face) = pm.f.get(f) {
                        s.extend(face.iter().copied());
                    }
                }
            }
        }
        s.into_iter().filter(|&v| (v as usize) < pm.v.len()).collect()
    }

    // ------------------------------------------------------------ undo
    /// Snapshot data for undo: the scene plus the JS `Undo.snapshot` extras (component mode,
    /// the hilite list and the component selections), so undoing an edit keeps component mode.
    fn selection_data(&self) -> Value {
        let comp: serde_json::Map<String, Value> = self.comp.iter().map(|(id, c)| (id.clone(), json!({ "v": c.v, "e": c.e, "f": c.f }))).collect();
        json!({
            "sel": self.sel,
            "compMode": self.comp_mode,
            "hilite": self.hilite,
            "compSel": Value::Object(comp),
        })
    }
    fn snapshot_data(&self) -> Value {
        let mut d = self.serialize();
        let o = d.as_object_mut().unwrap();
        o.insert("compMode".into(), serde_json::to_value(self.comp_mode).unwrap());
        o.insert("hilite".into(), json!(self.hilite));
        let comp: serde_json::Map<String, Value> = self.comp.iter().map(|(id, c)| (id.clone(), json!({ "v": c.v, "e": c.e, "f": c.f }))).collect();
        o.insert("compSel".into(), Value::Object(comp));
        d
    }
    fn push_undo(&mut self, label: &str, data: Value, selection_only: bool) {
        self.undo_stack.push(Snapshot { label: label.into(), data, selection_only });
        if self.undo_stack.len() > self.undo_limit {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }
    pub fn checkpoint(&mut self, label: &str) {
        let data = self.snapshot_data();
        self.push_undo(label, data, false);
    }
    /// Selection gestures only store the selection state, so clicking in a heavy scene does not
    /// serialize every mesh. The scene itself is unchanged by these steps.
    pub fn checkpoint_selection(&mut self, label: &str) {
        let data = self.selection_data();
        self.push_undo(label, data, true);
    }
    pub fn undo(&mut self) -> Option<String> {
        let s = self.undo_stack.pop()?;
        let cur = if s.selection_only { self.selection_data() } else { self.snapshot_data() };
        self.redo_stack.push(Snapshot { label: s.label.clone(), data: cur, selection_only: s.selection_only });
        if s.selection_only {
            self.restore_selection(&s.data);
        } else {
            self.restore(&s.data);
        }
        Some(s.label)
    }
    pub fn redo(&mut self) -> Option<String> {
        let s = self.redo_stack.pop()?;
        let cur = if s.selection_only { self.selection_data() } else { self.snapshot_data() };
        self.undo_stack.push(Snapshot { label: s.label.clone(), data: cur, selection_only: s.selection_only });
        if s.selection_only {
            self.restore_selection(&s.data);
        } else {
            self.restore(&s.data);
        }
        Some(s.label)
    }
    fn restore_selection(&mut self, d: &Value) {
        if let Some(sel) = d.get("sel").and_then(|v| v.as_array()) {
            self.sel = sel.iter().filter_map(|v| v.as_str().map(String::from)).filter(|id| self.nodes.contains_key(id)).collect();
        }
        self.comp_mode = d.get("compMode").and_then(|v| serde_json::from_value(v.clone()).ok()).flatten();
        if let Some(h) = d.get("hilite").and_then(|v| v.as_array()) {
            self.hilite = h.iter().filter_map(|v| v.as_str().map(String::from)).filter(|id| self.nodes.contains_key(id)).collect();
        }
        if let Some(cs) = d.get("compSel") {
            self.comp = serde_json::from_value::<HashMap<Id, CompSel>>(cs.clone()).unwrap_or_default();
            self.comp.retain(|id, _| self.nodes.contains_key(id));
        }
        self.rev += 1;
    }
    fn restore(&mut self, d: &Value) {
        let (file, project) = (self.file.clone(), self.project.clone());
        let undo = std::mem::take(&mut self.undo_stack);
        let redo = std::mem::take(&mut self.redo_stack);
        let comp_mode = self.comp_mode;
        let hilite = self.hilite.clone();
        let next_rev = self.nodes.values().map(|n| n.rev).max().unwrap_or(0).wrapping_add(1);
        let _ = self.load(d);
        // Keep the viewport and mesh caches invalidating across repeated undo/redo restores.
        for n in self.nodes.values_mut() {
            n.rev = next_rev;
        }
        // JS `Undo.restore`: component mode, the hilite and the component selections come back
        self.comp_mode = d.get("compMode").and_then(|v| serde_json::from_value(v.clone()).ok()).flatten().or(comp_mode);
        if let Some(h) = d.get("hilite").and_then(|v| v.as_array()) {
            self.hilite = h.iter().filter_map(|v| v.as_str().map(String::from)).filter(|id| self.nodes.contains_key(id)).collect();
        } else {
            self.hilite = hilite.into_iter().filter(|id| self.nodes.contains_key(id)).collect();
        }
        if let Some(cs) = d.get("compSel") {
            self.comp = serde_json::from_value::<HashMap<Id, CompSel>>(cs.clone()).unwrap_or_default();
            self.comp.retain(|id, _| self.nodes.contains_key(id));
        }
        self.undo_stack = undo;
        self.redo_stack = redo;
        self.file = file;
        self.project = project;
        self.modified = true;
    }

    // ------------------------------------------------------------ serialization
    pub fn serialize_node(&self, id: &str) -> Value {
        let n = &self.nodes[id];
        let mut d = json!({
            "id": n.id, "name": n.name, "kind": n.kind, "parent": n.parent,
            "t": n.xf.t, "r": n.xf.r, "s": n.xf.s, "ro": n.xf.ro, "p": n.xf.p, "pt": n.xf.pt,
            "visible": n.visible, "layer": n.layer, "template": n.template, "selectable": n.selectable, "xray": n.xray,
            "useOutlinerColor": n.use_outliner_color, "outlinerColor": n.outliner_color, "locks": n.locks,
            "shapeName": n.shape_name, "extra": n.extra,
        });
        let o = d.as_object_mut().unwrap();
        match &n.shape {
            Shape::Mesh(m) => {
                o.insert("material".into(), json!(m.material));
                o.insert("smoothLevel".into(), json!(m.smooth_level));
                o.insert("nurbs".into(), json!(m.nurbs));
                o.insert("history".into(), serde_json::to_value(self.history_view(id)).unwrap());
                o.insert("historyIds".into(), json!(m.history));
                o.insert("mesh".into(), serde_json::to_value(&m.mesh).unwrap());
            }
            Shape::Light(l) => {
                let mut lm = Map::new();
                lm.insert("type".into(), json!(l.ty));
                lm.extend(l.attrs.clone());
                o.insert("light".into(), Value::Object(lm));
            }
            Shape::Camera(c) => {
                o.insert("cam".into(), serde_json::to_value(c).unwrap());
            }
            Shape::Curve(c) => {
                o.insert("curve".into(), serde_json::to_value(c).unwrap());
            }
            _ => {}
        }
        if !n.published.is_empty() {
            o.insert("published".into(), serde_json::to_value(&n.published).unwrap());
        }
        if !n.hidden_channels.is_empty() {
            o.insert("hiddenChannels".into(), json!(n.hidden_channels));
        }
        if !n.nonkeyable.is_empty() {
            o.insert("nonkeyable".into(), json!(n.nonkeyable));
        }
        if !n.expressions.is_empty() {
            o.insert("expressions".into(), serde_json::to_value(&n.expressions).unwrap());
        }
        for (k, v) in &n.passthrough {
            o.insert(k.clone(), v.clone());
        }
        // the JS app reads rig nodes' keyable attributes from extraAttrs
        if (crate::rig::RIG_KINDS.contains(&n.kind.as_str()) || n.passthrough.contains_key("rig")) && n.extra.as_object().is_some_and(|e| !e.is_empty()) {
            o.insert("extraAttrs".into(), n.extra.clone());
        }
        if n.xf.jo != [0.0; 3] || n.kind == "joint" {
            o.insert("jo".into(), json!(n.xf.jo));
        }
        d
    }

    /// Create a node from its JSON. `fresh` = new ids/names (duplicate / import).
    pub fn deserialize_node(&mut self, d: &Value, parent: Option<&str>, fresh: bool) -> Id {
        let kind = d["kind"].as_str().unwrap_or("group").to_string();
        let name = d["name"].as_str().unwrap_or("node").to_string();
        let id = if fresh { None } else { d["id"].as_str().map(String::from) };
        let startup = d["cam"]["startup"].as_bool().unwrap_or(false);
        let shape = match kind.as_str() {
            "mesh" => {
                let mesh: PolyMesh = serde_json::from_value(d["mesh"].clone()).unwrap_or_default();
                let stored: Vec<Id> = d["historyIds"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).filter(|id| self.deps.node(id).is_some()).collect()).unwrap_or_default();
                let history = if stored.is_empty() {
                    let legacy: Vec<HistoryNode> = serde_json::from_value(d["history"].clone()).unwrap_or_default();
                    let mut ids = Vec::with_capacity(legacy.len());
                    for h in legacy {
                        let hid = if fresh || h.id.is_empty() { self.new_id("h") } else { h.id.clone() };
                        let name = if h.name.contains('#') { h.name } else if fresh { self.unique_name(&h.name) } else { self.claim(&h.name) };
                        let mut n = crate::graph::DepNode::new(&hid, &name, &h.ty);
                        for (k, v) in h.params {
                            n.add_attr(&k, crate::graph::AttrSpec::any(v));
                        }
                        self.deps.insert(n);
                        ids.push(hid);
                    }
                    ids
                } else {
                    stored
                };
                if let Some(hid) = history.last() {
                    if let Some(n) = self.deps.node_mut(hid) {
                        n.mesh_out = Some(mesh.clone());
                    }
                }
                let material = d["material"].as_str().filter(|m| self.is_material(m)).map(String::from).unwrap_or(self.default_mat.clone());
                Shape::Mesh(Box::new(MeshShape { material, smooth_level: d["smoothLevel"].as_u64().unwrap_or(0) as u32, nurbs: d["nurbs"].as_bool().unwrap_or(false), history, mesh }))
            }
            "light" => {
                let mut attrs = d["light"].as_object().cloned().unwrap_or_default();
                let ty = attrs.remove("type").and_then(|t| t.as_str().map(String::from)).unwrap_or("pointLight".into());
                let mut full = match light_defaults(&ty) {
                    Value::Object(m) => m,
                    _ => Map::new(),
                };
                full.extend(attrs);
                Shape::Light(LightShape { ty, attrs: full })
            }
            "camera" => Shape::Camera(serde_json::from_value(d["cam"].clone()).unwrap_or_default()),
            "curve" => Shape::Curve(serde_json::from_value(d["curve"].clone()).unwrap_or_default()),
            "locator" => Shape::Locator,
            "group" | "container" => Shape::Group,
            _ => Shape::Locator, // rig nodes from the JS app (joints, handles, ...) show as locators
        };
        let exact = !fresh && startup;
        let nid = if exact || !fresh {
            // loading a file: keep names exactly when free
            let nm = if exact { name.clone() } else { self.claim(&name) };
            self.make_dag(&kind, &nm, parent, shape, true, id)
        } else {
            self.make_dag(&kind, &name, parent, shape, false, id)
        };
        if !fresh {
            if let Some(sn) = d["shapeName"].as_str() {
                let n = self.nodes.get_mut(&nid).unwrap();
                if let Some(old) = n.shape_name.take() {
                    self.names.remove(&old);
                }
                let sn = self.claim(sn);
                self.nodes.get_mut(&nid).unwrap().shape_name = Some(sn);
            }
        }
        let n = self.nodes.get_mut(&nid).unwrap();
        let a3 = |v: &Value, def: [f64; 3]| -> [f64; 3] { serde_json::from_value(v.clone()).unwrap_or(def) };
        n.xf.t = a3(&d["t"], [0.0; 3]);
        n.xf.r = a3(&d["r"], [0.0; 3]);
        n.xf.s = a3(&d["s"], [1.0; 3]);
        n.xf.ro = d["ro"].as_str().unwrap_or("xyz").into();
        n.xf.p = a3(&d["p"], [0.0; 3]);
        n.xf.pt = a3(&d["pt"], [0.0; 3]);
        n.visible = d["visible"].as_bool().unwrap_or(true);
        n.layer = d["layer"].as_str().map(String::from);
        n.template = d["template"].as_bool().unwrap_or(false);
        n.selectable = d["selectable"].as_bool().unwrap_or(true);
        n.xray = d["xray"].as_bool().unwrap_or(false);
        n.use_outliner_color = d["useOutlinerColor"].as_bool().unwrap_or(false);
        n.outliner_color = serde_json::from_value(d["outlinerColor"].clone()).unwrap_or([1.0; 3]);
        n.locks = d["locks"].as_object().cloned().unwrap_or_default();
        n.hidden_channels = serde_json::from_value(d["hiddenChannels"].clone()).unwrap_or_default();
        n.nonkeyable = serde_json::from_value(d["nonkeyable"].clone()).unwrap_or_default();
        n.expressions = serde_json::from_value(d["expressions"].clone()).unwrap_or_default();
        n.extra = d["extra"].clone();
        // duplicated/imported nodes get new ids, so published targets would be stale
        n.published = if fresh { IndexMap::new() } else { serde_json::from_value(d["published"].clone()).unwrap_or_default() };
        for k in ["rig", "extraAttrs"] {
            if let Some(v) = d.get(k) {
                n.passthrough.insert(k.into(), v.clone());
            }
        }
        n.xf.jo = a3(&d["jo"], [0.0; 3]);
        if let Some(m) = n.mesh_mut() {
            bake_fm(m);
        }
        nid
    }

    /// Exact name if free, else the next unique one.
    fn claim(&mut self, n: &str) -> String {
        self.unique_name(n)
    }

    pub fn serialize(&self) -> Value {
        let nodes: Vec<Value> = self.all_dag(true).iter().map(|i| self.serialize_node(i)).collect();
        json!({
            "format": "inca-scene",
            "version": self.version,
            "idCounter": self.id_counter,
            "nodes": nodes,
            "deps": serde_json::to_value(&self.deps).unwrap_or(Value::Null),
            "shading": ShadingData { mats: self.materials(), texs: self.textures(), places: self.places(), default_mat_id: Some(self.default_mat.clone()) },
            "layers": self.layers,
            "anim": self.anim.serialize(),
            "animLayers": self.anim_layers,
            "animLayerCurrent": self.anim_layer_current,
            "animBaseLock": self.anim_base_lock,
            "animBaseGhost": self.anim_base_ghost,
            "animBaseGhostColor": self.anim_base_ghost_color,
            "animSoloSaved": self.anim_solo_saved,
            "animStatic": self.anim_static,
            "conns": self.conns,
            "constraints": self.constraints,
            "characters": if self.characters.is_empty() { Value::Null } else { serde_json::to_value(&self.characters).unwrap() },
            "time": TimeState { playing: false, ..self.time.clone() },
            "render": if self.render.is_empty() { Value::Null } else { Value::Object(self.render.clone()) },
            "sel": self.sel,
            "project": self.project,
        })
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.deps = Default::default();
        self.roots.clear();
        self.names.clear();
        self.layers.clear();
        self.current_layer = None;
        self.anim = Anim::default();
        self.anim_layers.clear();
        self.anim_layer_current = None;
        self.anim_base_lock = false;
        self.anim_base_ghost = false;
        self.anim_base_ghost_color = crate::anim::ghost_red();
        self.anim_solo_saved.clear();
        self.anim_static.clear();
        self.conns.clear();
        self.constraints.clear();
        self.characters.clear();
        self.dynamics = Default::default();
        self.sel.clear();
        self.hilite.clear();
        self.comp.clear();
        self.comp_mode = None;
        self.touch();
    }

    pub fn load(&mut self, data: &Value) -> Result<(), String> {
        if data.get("nodes").and_then(|n| n.as_array()).is_none() {
            return Err("not an Inca scene".into());
        }
        self.clear();
        self.id_counter = self.id_counter.max(data["idCounter"].as_u64().unwrap_or(1));
        self.deps = serde_json::from_value(data.get("deps").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        let shading: ShadingData = serde_json::from_value(data["shading"].clone()).unwrap_or_default();
        let migrated_shading = !self.deps.nodes.values().any(|n| crate::materials::is_material_kind(&n.kind));
        if migrated_shading {
            self.migrate_legacy_shading(shading.clone());
        }
        self.default_mat = shading.default_mat_id.filter(|d| self.is_material(d)).or_else(|| self.materials().first().map(|m| m.id.clone())).unwrap_or_default();
        if self.default_mat.is_empty() {
            self.init_default_materials();
        }
        if migrated_shading {
            if let Some(sg) = self.shading_group_of(&self.default_mat) {
                self.rename_shading_node(&sg, "initialShadingGroup");
            }
        }
        self.layers = serde_json::from_value(data["layers"].clone()).unwrap_or_default();
        for l in &self.layers {
            self.names.insert(l.name.clone());
        }
        for n in self.deps.nodes.values() {
            self.names.insert(n.name.clone());
        }
        for id in self.material_ids() {
            self.ensure_shading_engine(&id);
        }
        let mut by_id: HashMap<String, Id> = HashMap::new();
        for d in data["nodes"].as_array().unwrap() {
            let parent = d["parent"].as_str().and_then(|p| by_id.get(p).cloned());
            let id = self.deserialize_node(d, parent.as_deref(), false);
            by_id.insert(d["id"].as_str().unwrap_or("").to_string(), id);
        }
        if self.find("persp").is_none() {
            self.create_startup_cameras();
        }
        let anim: Vec<AnimCurve> = serde_json::from_value(data["anim"].clone()).unwrap_or_default();
        self.anim.load(anim);
        self.anim_layers = serde_json::from_value(data.get("animLayers").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        self.anim_layer_current = serde_json::from_value(data.get("animLayerCurrent").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        self.migrate_anim_layer_attrs();
        self.normalize_anim_layer_hierarchy();
        self.anim_base_lock = data.get("animBaseLock").and_then(|v| v.as_bool()).unwrap_or(false);
        self.anim_base_ghost = data.get("animBaseGhost").and_then(|v| v.as_bool()).unwrap_or(false);
        self.anim_base_ghost_color = serde_json::from_value(data.get("animBaseGhostColor").cloned().unwrap_or(Value::Null)).unwrap_or_else(|_| crate::anim::ghost_red());
        self.anim_solo_saved = serde_json::from_value(data.get("animSoloSaved").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        self.anim_static = serde_json::from_value(data.get("animStatic").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        self.conns = serde_json::from_value::<Vec<Conn>>(data["conns"].clone()).unwrap_or_default();
        self.constraints = serde_json::from_value::<Vec<Constraint>>(data["constraints"].clone()).unwrap_or_default();
        if let Ok(t) = serde_json::from_value::<TimeState>(data["time"].clone()) {
            self.time = TimeState { playing: false, ..t };
        }
        self.render = data["render"].as_object().cloned().unwrap_or_default();
        self.characters = serde_json::from_value(data["characters"].clone()).unwrap_or_default();
        self.sel = data["sel"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).filter(|i| self.nodes.contains_key(i)).collect()).unwrap_or_default();
        self.project = data["project"].as_str().map(String::from);
        self.import_js_rig();
        self.sync_display_layer_order();
        self.ensure_anim_layer_nodes();
        self.ensure_display_layer_nodes();
        // Restored/serialized nodes have no revision, but the viewport caches by it: a second
        // undo (or a load followed by undo) can otherwise keep showing the previous mesh.
        for n in self.nodes.values_mut() {
            n.rev = n.rev.wrapping_add(1);
        }
        self.touch();
        Ok(())
    }

    /// Fresh empty scene (File > New).
    pub fn reset(&mut self) {
        *self = Scene { project: self.project.take(), ..Scene::new() };
    }

    /// The effective display layer of a node, ignoring disabled layers. Unassigned objects
    /// belong to `defaultLayer`.
    pub fn layer_for(&self, id: &str) -> Option<&Layer> {
        let lid = self.nodes.get(id)?.layer.as_deref().unwrap_or("defaultLayer");
        self.layers.iter().find(|l| l.id == lid && self.display_layer_enabled(&l.id))
    }

    /// Rebuild `layerManager.displayLayers` in the current `Scene::layers` order.
    fn rebuild_display_layer_connections(&mut self) {
        let src = crate::graph::PlugRef::new("layerManager", "displayLayers");
        self.deps.connections.retain(|c| !(c.source == src && c.destination.attr == "layerManager"));
        for l in &self.layers {
            let dst = crate::graph::PlugRef::new(&l.id, "layerManager");
            if self.deps.node(&l.id).is_some() {
                self.deps.connect(src.clone(), dst);
            }
        }
    }

    /// Reorder the display-layer stack. The order of `layerManager.displayLayers` connections is
    /// authoritative; `Scene::layers` is kept as the UI mirror.
    pub fn set_display_layer_order(&mut self, ids: &[String]) {
        self.ensure_display_layer_nodes();
        let mut ordered: Vec<Layer> = Vec::with_capacity(self.layers.len());
        for id in ids.iter().filter(|id| id.as_str() != "defaultLayer") {
            if let Some(pos) = self.layers.iter().position(|l| &l.id == id) {
                ordered.push(self.layers.remove(pos));
            }
        }
        ordered.append(&mut self.layers);
        self.layers = ordered;
        if let Some(pos) = self.layers.iter().position(|l| l.id == "defaultLayer") {
            if pos + 1 < self.layers.len() {
                let l = self.layers.remove(pos);
                self.layers.push(l);
            }
        }
        self.rebuild_display_layer_connections();
        self.touch();
    }

    /// Adopt the manager's connection order into the UI layer list.
    fn sync_display_layer_order(&mut self) {
        let src = crate::graph::PlugRef::new("layerManager", "displayLayers");
        let order: Vec<String> = self.deps.connections.iter().filter(|c| c.source == src).map(|c| c.destination.node.clone()).collect();
        if order.is_empty() {
            return;
        }
        let mut ordered: Vec<Layer> = Vec::with_capacity(self.layers.len());
        for id in order {
            if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
                ordered.push(self.layers.remove(pos));
            }
        }
        ordered.append(&mut self.layers);
        self.layers = ordered;
    }

    /// Create or refresh the real `displayLayer` dependency node for every layer.
    pub fn ensure_display_layer_nodes(&mut self) {
        self.ensure_default_display_layer();
        self.ensure_layer_manager();
        let layers: Vec<Layer> = self.layers.clone();
        for l in &layers {
            if self.deps.node(&l.id).is_none() {
                let mut n = crate::graph::DepNode::new(&l.id, &l.name, "displayLayer");
                add_display_specs(&mut n);
                n.attrs.insert("visibility".into(), json!(l.visible));
                n.attrs.insert("displayType".into(), json!(l.mode));
                n.attrs.insert("playback".into(), json!(l.playback));
                n.attrs.insert("enabled".into(), json!(l.enabled));
                n.attrs.insert("shading".into(), json!(l.shading));
                n.attrs.insert("texturing".into(), json!(l.texturing));
                n.attrs.insert("lod".into(), json!(l.lod));
                n.attrs.insert("animPlayback".into(), json!(l.anim_playback));
                let rgb = l.color.as_ref().and_then(|v| v.as_array()).filter(|a| a.len() >= 3).map(|a| [a[0].as_f64().unwrap_or(0.5), a[1].as_f64().unwrap_or(0.5), a[2].as_f64().unwrap_or(0.5)]);
                n.attrs.insert("colorSet".into(), json!(rgb.is_some()));
                if let Some(rgb) = rgb {
                    n.attrs.insert("color".into(), json!(rgb));
                }
                self.deps.insert(n);
                self.names.insert(l.name.clone());
            } else {
                self.rename_dep_node(&l.id, &l.name);
                if let Some(n) = self.deps.node_mut(&l.id) {
                    add_display_specs(n);
                }
            }
        }
        // Each displayLayer's `layerManager` input comes from the singleton manager.
        self.rebuild_display_layer_connections();
        let live: std::collections::HashSet<String> = self.layers.iter().map(|l| l.id.clone()).collect();
        let orphans: Vec<String> = self.deps.nodes.values().filter(|n| n.kind == "displayLayer" && !live.contains(&n.id)).map(|n| n.id.clone()).collect();
        for id in orphans {
            self.remove_dep_node(&id);
        }
        for l in &mut self.layers {
            l.reset_legacy_state();
        }
    }

    fn display_layer_legacy<'a>(&'a self, id: &str) -> Option<&'a Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    fn display_layer_bool(&self, id: &str, attr: &str, legacy: impl Fn(&Layer) -> bool) -> bool {
        if let Some(v) = self.deps.node(id).and_then(|n| n.get(attr)) {
            return v.as_bool().unwrap_or_else(|| v.as_f64().is_some_and(|x| x != 0.0));
        }
        self.display_layer_legacy(id).map(legacy).unwrap_or(true)
    }

    pub fn display_layer_visible(&self, id: &str) -> bool {
        self.display_layer_bool(id, "visibility", |l| l.visible)
    }

    pub fn display_layer_playback(&self, id: &str) -> bool {
        self.display_layer_bool(id, "playback", |l| l.playback)
    }

    pub fn display_layer_enabled(&self, id: &str) -> bool {
        self.display_layer_bool(id, "enabled", |l| l.enabled)
    }

    pub fn display_layer_shading(&self, id: &str) -> bool {
        self.display_layer_bool(id, "shading", |l| l.shading)
    }

    pub fn display_layer_texturing(&self, id: &str) -> bool {
        self.display_layer_bool(id, "texturing", |l| l.texturing)
    }

    pub fn display_layer_anim_playback(&self, id: &str) -> bool {
        self.display_layer_bool(id, "animPlayback", |l| l.anim_playback)
    }

    pub fn display_layer_mode(&self, id: &str) -> String {
        if let Some(v) = self.deps.node(id).and_then(|n| n.get("displayType")) {
            return v.as_str().unwrap_or("normal").to_string();
        }
        self.display_layer_legacy(id).map(|l| l.mode.clone()).unwrap_or_else(|| "normal".into())
    }

    pub fn display_layer_lod(&self, id: &str) -> String {
        if let Some(v) = self.deps.node(id).and_then(|n| n.get("lod")) {
            return v.as_str().unwrap_or("full").to_string();
        }
        self.display_layer_legacy(id).map(|l| l.lod.clone()).unwrap_or_else(|| "full".into())
    }

    pub fn display_layer_color(&self, id: &str) -> Option<Value> {
        if let Some(v) = self.deps.node(id).and_then(|n| n.get("colorSet")) {
            if !v.as_bool().unwrap_or(false) {
                return None;
            }
            let c = self.deps.node(id).and_then(|n| n.get("color"))?;
            return c.is_array().then_some(c);
        }
        self.display_layer_legacy(id).and_then(|l| l.color.clone())
    }

    pub fn set_display_layer_bool(&mut self, id: &str, attr: &str, on: bool) -> bool {
        if id == "defaultLayer" && attr != "enabled" {
            return false;
        }
        self.ensure_display_layer_nodes();
        let ok = self.deps.set(&crate::graph::PlugRef::new(id, attr), json!(on));
        if ok {
            self.touch();
        }
        ok
    }

    pub fn set_display_layer_mode(&mut self, id: &str, mode: &str) {
        if id == "defaultLayer" {
            return;
        }
        self.ensure_display_layer_nodes();
        if self.deps.set(&crate::graph::PlugRef::new(id, "displayType"), json!(mode)) {
            self.touch();
        }
    }

    pub fn set_display_layer_lod(&mut self, id: &str, lod: &str) {
        if id == "defaultLayer" {
            return;
        }
        self.ensure_display_layer_nodes();
        if self.deps.set(&crate::graph::PlugRef::new(id, "lod"), json!(lod)) {
            self.touch();
        }
    }

    pub fn set_display_layer_color(&mut self, id: &str, color: Option<Value>) {
        if id == "defaultLayer" {
            return;
        }
        self.ensure_display_layer_nodes();
        let rgb = color.as_ref().and_then(|v| v.as_array()).filter(|a| a.len() >= 3).map(|a| [a[0].as_f64().unwrap_or(0.5), a[1].as_f64().unwrap_or(0.5), a[2].as_f64().unwrap_or(0.5)]);
        self.deps.set(&crate::graph::PlugRef::new(id, "colorSet"), json!(rgb.is_some()));
        if let Some(rgb) = rgb {
            self.deps.set(&crate::graph::PlugRef::new(id, "color"), json!(rgb));
        }
        self.touch();
    }

    /// Visible = node + all ancestors visible and the display layer visible.
    pub fn is_visible(&self, id: &str) -> bool {
        let mut c = Some(id.to_string());
        while let Some(i) = c {
            let Some(n) = self.nodes.get(&i) else { return false };
            if !n.visible {
                return false;
            }
            if let Some(lid) = n.layer.as_deref().or(Some("defaultLayer")) {
                if self.display_layer_enabled(lid) && (!self.display_layer_visible(lid) || (!self.display_layer_playback(lid) && self.playing)) {
                    return false;
                }
            }
            c = n.parent.clone();
        }
        true
    }

    /// Template (unselectable, grey) through the node, its ancestors or its layer.
    /// Templated (object or template display layer, not reference layers): drawn as gray wireframe.
    pub fn is_templated_display(&self, id: &str) -> bool {
        let mut c = Some(id.to_string());
        while let Some(i) = c {
            let Some(n) = self.nodes.get(&i) else { return false };
            let lid = n.layer.as_deref().unwrap_or("defaultLayer");
            if n.template || (self.display_layer_enabled(lid) && self.display_layer_mode(lid) == "template") {
                return true;
            }
            c = n.parent.clone();
        }
        false
    }

    pub fn is_template(&self, id: &str) -> bool {
        let mut c = Some(id.to_string());
        while let Some(i) = c {
            let Some(n) = self.nodes.get(&i) else { return false };
            if n.template {
                return true;
            }
            if let Some(l) = n.layer.as_deref().or(Some("defaultLayer")) {
                if self.display_layer_enabled(l) && self.display_layer_mode(l) != "normal" {
                    return true;
                }
            }
            c = n.parent.clone();
        }
        false
    }

    pub fn poly_count(&self) -> (usize, usize, usize, usize) {
        let (mut v, mut e, mut f, mut t) = (0, 0, 0, 0);
        for n in self.nodes.values() {
            if let Some(m) = n.mesh() {
                v += m.mesh.v.len();
                e += m.mesh.edge_count();
                f += m.mesh.f.len();
                t += m.mesh.tri_count();
            }
        }
        (v, e, f, t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    Replace,
    Add,
    Toggle,
    Remove,
}

impl OpCtx for Scene {
    fn world_curve(&self, id: &str) -> Option<Curve> {
        match &self.nodes.get(id)?.shape {
            Shape::Curve(c) => Some(c.transformed(&self.world_matrix(id))),
            _ => None,
        }
    }
    fn world_mesh(&self, id: &str) -> Option<PolyMesh> {
        self.mesh_world(id)
    }
    fn world_matrix(&self, id: &str) -> Option<DMat4> {
        self.nodes.contains_key(id).then(|| Scene::world_matrix(self, id))
    }
    fn node_rig(&self, id: &str) -> Option<Value> {
        self.nodes.get(id)?.passthrough.get("rig").cloned()
    }
    fn node_extra(&self, id: &str, key: &str) -> Option<f64> {
        let v = self.nodes.get(id)?.extra.get(key)?;
        v.as_f64().or_else(|| v.as_bool().map(|b| b as i32 as f64))
    }
    fn local_verts(&self, id: &str) -> Option<Vec<DVec3>> {
        self.nodes.get(id)?.mesh().map(|m| m.mesh.v.clone())
    }
}

/// Per-face materials equal to the object material are stored as None.
fn bake_fm(m: &mut MeshShape) {
    let mat = m.material.clone();
    if let Some(fm) = &mut m.mesh.fm {
        for x in fm.iter_mut() {
            if x.as_deref() == Some(mat.as_str()) {
                *x = None;
            }
        }
        if fm.iter().all(|x| x.is_none()) {
            m.mesh.fm = None;
        }
    }
}

fn xf_attr(a: &str) -> Option<(&'static str, usize)> {
    let k = match a.get(..a.len().saturating_sub(1))? {
        "translate" => "t",
        "rotate" => "r",
        "scale" => "s",
        "rotatePivot" | "scalePivot" => "p",
        "rotateAxis" => "ra",
        _ => return None,
    };
    let i = match a.chars().last()? {
        'X' => 0,
        'Y' => 1,
        'Z' => 2,
        _ => return None,
    };
    Some((k, i))
}
fn xf_vec<'a>(x: &'a Xform, k: &str) -> &'a [f64; 3] {
    match k {
        "t" => &x.t,
        "r" => &x.r,
        "s" => &x.s,
        "ra" => &x.ra,
        _ => &x.p,
    }
}
fn xf_vec_mut<'a>(x: &'a mut Xform, k: &str) -> &'a mut [f64; 3] {
    match k {
        "t" => &mut x.t,
        "r" => &mut x.r,
        "s" => &mut x.s,
        "ra" => &mut x.ra,
        _ => &mut x.p,
    }
}

/// `color` returns the vector, `colorR`/`colorG`/`colorB` a component.
fn get_compound(m: &Map<String, Value>, attr: &str) -> Option<Value> {
    if let Some(v) = m.get(attr) {
        return Some(v.clone());
    }
    let (base, idx) = split_component(attr)?;
    m.get(base)?.as_array()?.get(idx).cloned()
}
fn set_compound(m: &mut Map<String, Value>, attr: &str, v: Value) -> bool {
    if m.contains_key(attr) {
        let old = &m[attr];
        let v = match (old, &v) {
            (Value::Bool(_), Value::Number(n)) => Value::Bool(n.as_f64().unwrap_or(0.0) != 0.0),
            _ => v,
        };
        m.insert(attr.into(), v);
        return true;
    }
    if let Some((base, idx)) = split_component(attr) {
        if let Some(Value::Array(a)) = m.get_mut(base) {
            if idx < a.len() {
                a[idx] = v;
                return true;
            }
        }
    }
    false
}
fn split_component(attr: &str) -> Option<(&str, usize)> {
    let last = attr.chars().last()?;
    let idx = match last {
        'R' | 'X' => 0,
        'G' | 'Y' => 1,
        'B' | 'Z' => 2,
        _ => return None,
    };
    Some((&attr[..attr.len() - 1], idx))
}

pub fn light_defaults(ty: &str) -> Value {
    match ty {
        "ambientLight" => json!({ "color": [1, 1, 1], "intensity": 1, "ambientShade": 0.45 }),
        "directionalLight" => json!({ "color": [1, 1, 1], "intensity": 1, "shadows": false, "shadowRadius": 2, "shadowColor": [0, 0, 0] }),
        "pointLight" => json!({ "color": [1, 1, 1], "intensity": 1, "decayRate": 0, "shadows": false, "shadowRadius": 2 }),
        "spotLight" => json!({ "color": [1, 1, 1], "intensity": 1, "decayRate": 0, "coneAngle": 40, "penumbraAngle": 0, "dropoff": 0, "shadows": false, "shadowRadius": 2 }),
        "areaLight" => json!({ "color": [1, 1, 1], "intensity": 1, "decayRate": 2, "shadows": false }),
        "skyDomeLight" => json!({ "color": [1, 1, 1], "intensity": 1, "textureFile": "", "showBackground": true }),
        _ => json!({}),
    }
}

pub const LIGHT_TYPES: &[(&str, &str)] = &[
    ("ambientLight", "Ambient Light"),
    ("directionalLight", "Directional Light"),
    ("pointLight", "Point Light"),
    ("spotLight", "Spot Light"),
    ("areaLight", "Area Light"),
    ("skyDomeLight", "Sky Dome Light"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn set_layer_weight(s: &mut Scene, index: usize, weight: f64) {
        let id = s.anim_layers[index].id.clone();
        s.set_anim_layer_weight(&id, weight);
    }

    #[test]
    fn transform_shear_rotate_axis_and_inheritance() {
        let mut s = Scene::new();
        let parent = s.create_mesh("polyCube", &Params::new(), None);
        let child = s.create_mesh("polyCube", &Params::new(), None);
        assert!(s.reparent(&child, Some(&parent), None));
        let p = NodeRef::Dag(parent.clone());
        let c = NodeRef::Dag(child.clone());
        s.set_attr(&p, "translateX", json!(5.0));
        s.set_attr(&c, "inheritsTransform", json!(0));
        assert!((s.world_matrix(&child).w_axis.x - 0.0).abs() < 1e-9, "child should ignore the parent");
        s.set_attr(&c, "inheritsTransform", json!(1));
        assert!((s.world_matrix(&child).w_axis.x - 5.0).abs() < 1e-9, "child should inherit the parent");
        s.set_attr(&p, "translateX", json!(0.0));
        s.set_attr(&c, "shearXY", json!(2.0));
        let sheared = s.local_matrix(&child).transform_point3(DVec3::new(0.0, 1.0, 0.0));
        assert!((sheared.x - 2.0).abs() < 1e-9 && (sheared.y - 1.0).abs() < 1e-9, "shearXY: {sheared:?}");
        // A rotate axis on its own must not rotate the object; it only tilts the rotation axes.
        s.set_attr(&c, "shearXY", json!(0.0));
        s.set_attr(&c, "rotateAxisX", json!(45.0));
        let unrotated = s.local_matrix(&child).transform_point3(DVec3::new(1.0, 0.0, 0.0));
        assert!((unrotated - DVec3::new(1.0, 0.0, 0.0)).length() < 1e-9, "rotateAxis alone rotated the object: {unrotated:?}");
        s.set_attr(&c, "rotateY", json!(90.0));
        let tilted = s.local_matrix(&child).transform_point3(DVec3::new(0.0, 0.0, 1.0));
        let k = 0.5_f64.sqrt();
        assert!((tilted - DVec3::new(k, 0.5, 0.5)).length() < 1e-6, "rotate axis did not affect the rotation: {tilted:?}");

        // set_local_matrix must recover shear and rotate axis, not just scale/rotation.
        let mut want = Scene::new();
        let n = want.create_mesh("polyCube", &Params::new(), None);
        assert!(want.set_attr(&NodeRef::Dag(n.clone()), "rotateAxisX", json!(5.0)));
        assert!(want.set_attr(&NodeRef::Dag(n.clone()), "rotateAxisY", json!(6.0)));
        assert!(want.set_attr(&NodeRef::Dag(n.clone()), "rotateAxisZ", json!(7.0)));
        let xf = Xform {
            t: [1.0, 2.0, 3.0],
            r: [10.0, 20.0, 30.0],
            s: [2.0, 3.0, 4.0],
            sh: [0.2, 0.3, 0.4],
            ra: [5.0, 6.0, 7.0],
            ..Default::default()
        };
        let m = xf.matrix();
        want.set_local_matrix(&n, m);
        let got = want.world_matrix(&n);
        let (a, b) = (m.to_cols_array(), got.to_cols_array());
        for i in 0..16 {
            assert!((a[i] - b[i]).abs() < 1e-5, "matrix element {i}: {} vs {}", a[i], b[i]);
        }
    }

    #[test]
    fn transform_offset_parent_matrix_and_limits() {
        let mut s = Scene::new();
        let parent = s.create_mesh("polyCube", &Params::new(), None);
        let child = s.create_mesh("polyCube", &Params::new(), None);
        assert!(s.reparent(&child, Some(&parent), None));
        let p = NodeRef::Dag(parent.clone());
        let c = NodeRef::Dag(child.clone());
        s.set_attr(&p, "translateX", json!(10.0));
        let mut opm = identity4();
        opm[3][0] = 5.0;
        assert!(s.set_attr(&c, "offsetParentMatrix", json!(opm)));
        assert!((s.world_matrix(&child).w_axis.x - 15.0).abs() < 1e-9, "OPM not applied between parent and local");
        s.set_world_matrix(&child, DMat4::from_translation(DVec3::new(20.0, 0.0, 0.0)));
        assert!((s.world_matrix(&child).w_axis.x - 20.0).abs() < 1e-9, "set_world_matrix did not invert the OPM");
        // Limits clamp transform channels without affecting the scripted attribute access.
        assert!(s.set_attr(&c, "limitXEnable", json!(1)));
        assert!(s.set_attr(&c, "limitXMin", json!(-1.0)));
        assert!(s.set_attr(&c, "limitXMax", json!(1.0)));
        s.set_attr(&c, "translateX", json!(5.0));
        assert!((s.get_attr(&c, "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0) - 1.0).abs() < 1e-9);
        s.set_attr(&c, "translateX", json!(-5.0));
        assert!((s.get_attr(&c, "translateX").and_then(|v| v.as_f64()).unwrap_or(0.0) + 1.0).abs() < 1e-9);
        assert!((s.get_attr(&c, "limitXMin").and_then(|v| v.as_f64()).unwrap_or(0.0) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn history_params_are_not_aliased() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let mut p = Params::new();
        p.insert("edge".into(), json!([0, 1]));
        p.insert("t".into(), json!(0.5));
        s.apply_op(&id, "polySplitRing", p, true);
        let r = NodeRef::History(id.clone(), 1);
        assert!(s.set_attr(&r, "t", json!(0.25)));
        assert_eq!(s.get_attr(&r, "t"), Some(json!(0.25)));
    }

    #[test]
    fn names_increment_like_maya() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let b = s.create_mesh("polyCube", &Params::new(), None);
        assert_eq!(s.name_of(&a), "pCube1");
        assert_eq!(s.name_of(&b), "pCube2");
        assert_eq!(s.nodes[&a].shape_name.as_deref(), Some("pCubeShape1"));
        assert_eq!(s.history_node(&a, 0).unwrap().name, "polyCube1");
    }

    #[test]
    fn light_defaults_match_js() {
        let mut s = Scene::new();
        let d = s.create_light("directionalLight");
        let p = s.create_light("pointLight");
        let sp = s.create_light("spotLight");
        assert_eq!(s.nodes[&d].xf.r, [-45.0, 45.0, 0.0]);
        assert_eq!(s.nodes[&p].xf.r, [0.0, 0.0, 0.0]);
        assert_eq!(s.nodes[&sp].xf.r, [-45.0, 45.0, 0.0]);
        assert_eq!(s.get_attr(&NodeRef::Dag(d), "shadowRadius"), Some(json!(2)));
        assert_eq!(s.get_attr(&NodeRef::Dag(sp), "shadowRadius"), Some(json!(2)));
    }

    #[test]
    fn history_param_reevaluates() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let r = s.resolve("polyCube1").unwrap();
        assert!(s.set_attr(&r, "subdivisionsWidth", json!(3)));
        assert_eq!(s.nodes[&a].mesh().unwrap().mesh.f.len(), 14);
    }

    #[test]
    fn pivot_and_matrix() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_attr_f(&a, "tx", 2.0);
        s.set_pivot_world(&a, DVec3::new(3.0, 0.0, 0.0));
        let before = s.world_matrix(&a).transform_point3(DVec3::ZERO);
        assert!((before - DVec3::new(2.0, 0.0, 0.0)).length() < 1e-9);
        s.set_attr_f(&a, "ry", 180.0);
        let after = s.world_matrix(&a).transform_point3(DVec3::ZERO);
        assert!((after - DVec3::new(4.0, 0.0, 0.0)).length() < 1e-6, "{after}");
    }

    #[test]
    fn soft_angle_attr_round_trips() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let r = NodeRef::Dag(a.clone());
        assert_eq!(s.get_attr(&r, "softAngle"), Some(json!(60.0)));
        assert!(s.set_attr(&r, "softAngle", json!(15.0)));
        assert_eq!(s.nodes[&a].mesh().unwrap().mesh.soft_angle, 15.0);
        assert_eq!(s.get_attr(&r, "softAngle"), Some(json!(15.0)));
        // clamped to the JS range
        assert!(s.set_attr(&r, "softAngle", json!(500.0)));
        assert_eq!(s.get_attr(&r, "softAngle"), Some(json!(180.0)));
    }

    #[test]
    fn unselectable_nodes_never_enter_the_selection_but_templates_do() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.nodes.get_mut(&a).unwrap().selectable = false;
        s.select(std::slice::from_ref(&a), SelMode::Replace);
        assert!(s.sel.is_empty());
        s.nodes.get_mut(&a).unwrap().selectable = true;
        s.select(std::slice::from_ref(&a), SelMode::Replace);
        assert_eq!(s.sel, vec![a.clone()]);
        // Templated nodes stay selectable from the Outliner/Hypergraph; only the viewport blocks them.
        s.nodes.get_mut(&a).unwrap().template = true;
        s.select(std::slice::from_ref(&a), SelMode::Replace);
        assert_eq!(s.sel, vec![a.clone()]);
        // the flag round-trips (JS keeps it on the object only until the scene is saved)
        s.nodes.get_mut(&a).unwrap().template = false;
        s.nodes.get_mut(&a).unwrap().selectable = false;
        let data = s.serialize();
        let mut s2 = Scene::new();
        s2.load(&data).unwrap();
        assert!(!s2.node(&a).unwrap().selectable);
    }

    #[test]
    fn object_xray_round_trips_and_undoes() {
        let mut s = Scene::new();
        let a = s.create_mesh("polySphere", &Params::new(), None);
        s.checkpoint("object xray");
        s.nodes.get_mut(&a).unwrap().xray = true;
        let data = s.serialize();
        let mut s2 = Scene::new();
        s2.load(&data).unwrap();
        assert!(s2.node(&a).unwrap().xray);
        s.undo();
        assert!(!s.node(&a).unwrap().xray);
    }

    #[test]
    fn round_trip_and_undo() {
        let mut s = Scene::new();
        let a = s.create_mesh("polySphere", &Params::new(), None);
        s.set_attr_f(&a, "ty", 3.0);
        s.create_light("pointLight");
        let data = s.serialize();
        let mut s2 = Scene::new();
        s2.load(&data).unwrap();
        assert_eq!(s2.serialize(), data);
        s.checkpoint("move");
        s.set_attr_f(&a, "ty", 5.0);
        s.undo();
        assert_eq!(s.get_attr_f(&a, "ty"), 3.0);
        s.redo();
        assert_eq!(s.get_attr_f(&a, "ty"), 5.0);
    }

    #[test]
    fn rotation_channels_continue_past_360() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        for deg in (10..=720).step_by(10) {
            s.set_local_matrix(&a, DMat4::from_rotation_y((deg as f64).to_radians()));
            assert!((s.get_attr_f(&a, "ry") - deg as f64).abs() < 1e-6, "at {deg}: ry = {}", s.get_attr_f(&a, "ry"));
        }
    }

    #[test]
    fn direct_connections_follow_changes_live() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let b = s.create_mesh("polyCone", &Params::new(), None);
        let c = s.create_mesh("polySphere", &Params::new(), None);
        s.set_attr_f(&a, "ry", 30.0);
        // inserted out of order: evaluating a -> b must also feed b -> c in the same call
        s.conns.push(Conn { src: b.clone(), src_attr: "rotateY".into(), dst: c.clone(), dst_attr: "rotateY".into() });
        s.conns.push(Conn { src: a.clone(), src_attr: "rotateY".into(), dst: b.clone(), dst_attr: "rotateY".into() });
        assert_eq!(s.get_attr_f(&b, "ry"), 0.0);
        let moved = s.eval_connections();
        assert!(moved.contains(&b) && moved.contains(&c));
        assert_eq!(s.get_attr_f(&b, "ry"), 30.0);
        assert_eq!(s.get_attr_f(&c, "ry"), 30.0);
        // a source change is picked up on the next evaluation (the app runs one per frame)
        s.set_attr_f(&a, "ry", 75.0);
        s.eval_connections();
        assert_eq!(s.get_attr_f(&b, "ry"), 75.0);
        assert_eq!(s.get_attr_f(&c, "ry"), 75.0);
        // settled values report no movement
        assert!(s.eval_connections().is_empty());
    }

    #[test]
    fn parenting_keeps_world() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let g = s.create_group("group1", None);
        s.set_attr_f(&g, "tx", 5.0);
        s.set_attr_f(&g, "ry", 90.0);
        s.set_attr_f(&a, "tz", 1.0);
        let w = s.world_matrix(&a).transform_point3(DVec3::ZERO);
        s.reparent(&a, Some(&g), None);
        let w2 = s.world_matrix(&a).transform_point3(DVec3::ZERO);
        assert!((w - w2).length() < 1e-6);
        let d = s.duplicate(std::slice::from_ref(&g), false);
        assert_eq!(s.name_of(&d[0]), "group2");
        assert_eq!(s.nodes[&d[0]].children.len(), 1);
    }

    #[test]
    fn animation_drives_channels() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "tx", Some(1.0), Some(0.0));
        s.set_key(&a, "tx", Some(11.0), Some(10.0));
        s.set_time(11.0);
        assert!((s.get_attr_f(&a, "tx") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn animation_layers_blend_and_solo() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        let mut c = AnimCurve::new(&a, "translateX");
        c.set_key(0.0, 20.0, "linear");
        c.set_key(10.0, 20.0, "linear");
        let mut curves = IndexMap::new();
        curves.insert(Anim::key(&a, "translateX"), c);
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![],
            attrs: vec![],
            parent: None,
            curves,
        });
        s.set_anim_layer_override("al1", true);
        s.set_anim_layer_passthrough("al1", false);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 20.0).abs() < 1e-9, "override layer wins");
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 12.5).abs() < 1e-9, "weight blends");
        s.set_anim_layer_bool("al1", "mute", true);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 5.0).abs() < 1e-9, "mute returns to base");
        s.set_anim_layer_bool("al1", "mute", false);
        s.set_anim_layer_bool("al1", "solo", true);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 12.5).abs() < 1e-9, "solo keeps only that layer");
    }

    #[test]
    fn keys_go_into_the_current_animation_layer() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![Anim::key(&a, "translateX")],
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        s.set_time(3.0);
        s.set_key(&a, "translateX", Some(3.0), Some(7.0));
        assert!(s.anim.curves[&Anim::key(&a, "translateX")].find(3.0).is_none(), "key must not go to the base");
        let c = layer_curve(&s, "al1", &a, "translateX");
        let base = s.anim.curves[&Anim::key(&a, "translateX")].eval(3.0);
        assert!((c.eval(3.0) - (7.0 - base)).abs() < 1e-9, "additive key stores the delta");
        s.set_time(3.0);
        assert!((s.get_attr_f(&a, "translateX") - 7.0).abs() < 1e-9);
        assert!(s.anim_key_times(std::slice::from_ref(&a)).contains(&3.0), "timeline sees layer keys");
        assert!(s.anim_curve(&a, "translateX").is_some_and(|c| c.find(3.0).is_some()));
        // locked layers reject keys
        s.set_anim_layer_bool("al1", "lock", true);
        s.set_key(&a, "translateX", Some(4.0), Some(99.0));
        assert!(layer_curve(&s, "al1", &a, "translateX").find(4.0).is_none());
        assert!(s.anim.curves[&Anim::key(&a, "translateX")].find(4.0).is_none());
    }

    fn layer_curve<'a>(s: &'a Scene, layer: &str, node: &str, attr: &str) -> &'a AnimCurve {
        s.anim_layer_curve(layer, &Anim::key(node, attr)).expect("layer curve")
    }

    fn anim_test_layer(s: &mut Scene, id: &str, node: &str, attr: &str, override_mode: bool, pts: &[(f64, f64)]) -> AnimLayer {
        let mut c = AnimCurve::new(node, attr);
        for (t, v) in pts {
            c.set_key(*t, *v, "linear");
        }
        let mut curves = IndexMap::new();
        let key = Anim::key(node, attr);
        curves.insert(key.clone(), c);
        s.ensure_anim_layer_node(id, id);
        s.set_anim_layer_override(id, override_mode);
        s.set_anim_layer_passthrough(id, false);
        AnimLayer {
            id: id.into(),
            name: id.into(),
            weight_curve: Default::default(),
            members: vec![node.into()],
            attrs: vec![key],
            parent: None,
            curves,
        }
    }

    fn baked_values(s: &mut Scene, node: &str, attr: &str, times: &[f64]) -> Vec<f64> {
        times
            .iter()
            .map(|t| {
                s.set_time(*t);
                s.get_attr_f(node, attr)
            })
            .collect()
    }

    #[test]
    fn merge_layer_down_preserves_animation() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        s.anim_layers = vec![
            anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 2.0), (10.0, -1.0)]),
            anim_test_layer(&mut s, "al2", &a, "translateX", false, &[(0.0, 3.0), (10.0, 8.0)]),
        ];
        let times = [0.0, 5.0, 10.0];
        let before = baked_values(&mut s, &a, "translateX", &times);
        assert!(s.merge_anim_layers("al2", &["al1".into()]), "merge failed");
        assert_eq!(s.anim_layers.len(), 1);
        assert_eq!(s.anim_layer_current.as_deref(), Some("al2"));
        let after = baked_values(&mut s, &a, "translateX", &times);
        for (b, a) in before.iter().zip(after.iter()) {
            assert!((b - a).abs() < 1e-9, "merge changed the animation: {before:?} vs {after:?}");
        }
    }

    #[test]
    fn merge_with_override_source_uses_override_result() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        s.anim_layers = vec![
            anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 2.0), (10.0, -1.0)]),
            anim_test_layer(&mut s, "al2", &a, "translateX", true, &[(0.0, 3.0), (10.0, 8.0)]),
        ];
        let times = [0.0, 4.0, 10.0];
        let before = baked_values(&mut s, &a, "translateX", &times);
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        assert!(s.anim_layer_override("al1"), "an override source must make the result override");
        let after = baked_values(&mut s, &a, "translateX", &times);
        for (b, a) in before.iter().zip(after.iter()) {
            assert!((b - a).abs() < 1e-9, "merge changed the animation: {before:?} vs {after:?}");
        }
    }

    #[test]
    fn merge_options_can_exclude_children_and_keep_empty_sources() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let target = anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 1.0)]);
        let source = anim_test_layer(&mut s, "al2", &a, "translateY", false, &[(0.0, 2.0)]);
        let mut child = anim_test_layer(&mut s, "al3", &a, "translateZ", false, &[(0.0, 3.0)]);
        child.parent = Some("al2".into());
        s.anim_layers = vec![target, source, child];
        let opts = MergeLayerOptions { include_children: false, ..Default::default() };
        assert!(s.merge_anim_layers_with("al1", &["al2".into()], opts));
        assert_eq!(s.anim_layers.len(), 3, "the emptied parent must stay for its child");
        let kept = s.anim_layers.iter().find(|l| l.id == "al2").expect("kept parent");
        assert!(kept.attrs.is_empty());
        assert!(s.anim_layer_curve("al3", &Anim::key(&a, "translateZ")).is_some());
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateY") - 2.0).abs() < 1e-9, "source animation was not merged");
        assert!((s.get_attr_f(&a, "translateZ") - 3.0).abs() < 1e-9, "unselected child animation was merged");
    }

    #[test]
    fn merge_options_force_result_mode_and_keep_sources() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![
            anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 1.0)]),
            anim_test_layer(&mut s, "al2", &a, "translateX", true, &[(0.0, 5.0)]),
        ];
        let opts = MergeLayerOptions { result_mode: MergeLayerMode::Additive, delete_layers: false, ..Default::default() };
        s.set_time(0.0);
        let before = s.get_attr_f(&a, "translateX");
        assert!(s.merge_anim_layers_with("al1", &["al2".into()], opts));
        assert!(!s.anim_layer_override("al1"), "Additive must override Automatic");
        assert_eq!(s.anim_layers.len(), 2);
        assert!(s.anim_layers.iter().find(|l| l.id == "al2").expect("kept source").curves.is_empty());
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - before).abs() < 1e-9);
    }

    #[test]
    fn override_passthrough_blends_with_layers_below() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let lower = anim_test_layer(&mut s, "al2", &a, "translateX", true, &[(0.0, 10.0)]);
        let upper = anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 30.0)]);
        s.anim_layers = vec![upper, lower];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - 15.0).abs() < 1e-9, "opaque override should ignore the layer below");
        s.set_anim_layer_passthrough("al1", true);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - 20.0).abs() < 1e-9, "passthrough should blend with the layer below");
        set_layer_weight(&mut s, 0, 0.0);
        s.set_anim_layer_passthrough("al1", false);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX")).abs() < 1e-9, "opaque override at weight 0 must still block lower layers");
        s.set_anim_layer_passthrough("al1", true);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - 10.0).abs() < 1e-9, "passthrough at weight 0 exposes the layer below");
    }

    #[test]
    fn extract_base_animation_moves_curves_and_preserves_the_pose() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let b = s.create_mesh("polyCube", &Params::new(), None);
        for id in [&a, &b] {
            s.set_key(id, "translateX", Some(0.0), Some(0.0));
            s.set_key(id, "translateX", Some(10.0), Some(10.0));
        }
        s.set_time(5.0);
        let id = s.extract_base_animation(std::slice::from_ref(&a), "BaseAnimation_extract").expect("extract");
        assert_eq!(s.anim_layer_current.as_deref(), Some(id.as_str()));
        let layer = &s.anim_layers[0];
        assert_eq!(layer.name, "BaseAnimation_extract");
        assert!(s.anim_layer_override(&layer.id) && s.anim_layer_passthrough(&layer.id));
        assert!(layer.attrs.contains(&Anim::key(&a, "translateX")));
        assert!(!s.anim.curves.contains_key(&Anim::key(&a, "translateX")), "base curve was not moved");
        assert!(s.anim.curves.contains_key(&Anim::key(&b, "translateX")), "other object's base curve moved");
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 5.0).abs() < 1e-9, "extraction changed the pose");
        assert!((s.get_attr_f(&b, "translateX") - 5.0).abs() < 1e-9);
        let second = s.extract_base_animation(std::slice::from_ref(&b), "BaseAnimation_extract").expect("extract all");
        assert_eq!(s.anim_layers[0].name, "BaseAnimation_extract1");
        assert_eq!(s.anim_layer_current.as_deref(), Some(second.as_str()));
        assert!(s.anim.curves.is_empty());
    }

    #[test]
    fn extract_base_animation_returns_none_without_animation() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        assert!(s.extract_base_animation(&[a], "BaseAnimation_extract").is_none());
        assert!(s.anim_layers.is_empty());
    }

    #[test]
    fn keyed_layer_weight_blends_over_time() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut layer = anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 10.0)]);
        layer.weight_curve.set_key(0.0, 0.0, "linear");
        layer.weight_curve.set_key(10.0, 1.0, "linear");
        s.anim_layers = vec![layer];
        for (t, want) in [(0.0, 0.0), (5.0, 5.0), (10.0, 10.0)] {
            s.set_time(t);
            assert!((s.get_attr_f(&a, "translateX") - want).abs() < 1e-9, "time {t}");
        }
        let layer = &s.anim_layers[0];
        assert!((s.anim_layer_effective_weight_at(layer, 5.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn graph_editor_reads_and_edits_layer_weight_curves() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut layer = anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 10.0)]);
        layer.weight_curve.set_key(0.0, 0.0, "linear");
        layer.weight_curve.set_key(10.0, 1.0, "linear");
        s.anim_layers = vec![layer];
        s.anim_layer_current = Some("al1".into());
        let key = Scene::layer_weight_curve_key("al1");
        assert_eq!(s.graph_curve(&key).expect("weight curve").keys.len(), 2);
        assert!(s.graph_curve(&Anim::key(&a, "translateX")).is_some(), "normal curves still resolve");
        s.graph_curve_mut(&key).unwrap().keys[0].v = 0.5;
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - 5.0).abs() < 1e-9, "edited weight keys affect the pose");
    }

    #[test]
    fn merge_smart_bake_and_sample_by_control_key_times() {
        let mut smart_scene = Scene::new();
        let a = smart_scene.create_mesh("polyCube", &Params::new(), None);
        smart_scene.anim_layers = vec![
            anim_test_layer(&mut smart_scene, "al1", &a, "translateX", false, &[(0.0, 2.0), (5.0, 4.0), (10.0, 2.0)]),
            anim_test_layer(&mut smart_scene, "al2", &a, "translateX", false, &[(0.0, 0.0), (5.0, 1.0), (10.0, 0.0)]),
        ];
        let key = Anim::key(&a, "translateX");
        let smart = MergeLayerOptions { smart_bake: true, ..Default::default() };
        assert!(smart_scene.merge_anim_layers_with("al1", &["al2".into()], smart));
        let times: Vec<f64> = layer_curve(&smart_scene, "al1", &a, "translateX").keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![0.0, 5.0, 10.0], "Smart Bake should keep only source key times");
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![
            anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 2.0), (5.0, 4.0), (10.0, 2.0)]),
            anim_test_layer(&mut s, "al2", &a, "translateX", false, &[(0.0, 0.0), (5.0, 1.0), (10.0, 0.0)]),
        ];
        let sampled = MergeLayerOptions { smart_bake: false, sample_by: 2, ..Default::default() };
        assert!(s.merge_anim_layers_with("al1", &["al2".into()], sampled));
        let times: Vec<f64> = layer_curve(&s, "al1", &a, "translateX").keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![0.0, 2.0, 4.0, 5.0, 6.0, 8.0, 10.0], "Sample By 2 should bake every second frame plus source keys");
    }

    #[test]
    fn merge_increase_fidelity_adds_keys_between_spline_keys() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut target = anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 0.0), (5.0, 10.0), (10.0, 0.0)]);
        let key = Anim::key(&a, "translateX");
        for k in &mut target.curves.get_mut(&key).unwrap().keys {
            k.it = "spline".into();
            k.ot = "spline".into();
        }
        s.anim_layers = vec![target, anim_test_layer(&mut s, "al2", &a, "translateX", false, &[(0.0, 0.0), (10.0, 0.0)])];
        let opts = MergeLayerOptions { smart_bake: true, increase_fidelity: true, fidelity_tolerance: 1, ..Default::default() };
        assert!(s.merge_anim_layers_with("al1", &["al2".into()], opts));
        assert!(layer_curve(&s, "al1", &a, "translateX").keys.len() > 3, "Increase Fidelity did not add keys");
    }

    #[test]
    fn anim_layer_weight_is_stored_on_its_node() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 1.0)])];
        s.set_time(0.0);
        let layer_ref = NodeRef::Dependency("al1".into());
        assert_eq!(s.get_attr(&layer_ref, "weight"), Some(json!(1.0)), "layer node was not created");
        assert!(s.set_anim_layer_weight("al1", 0.25));
        assert_eq!(s.get_attr(&layer_ref, "weight"), Some(json!(0.25)));
        assert!((s.anim_layer_effective_weight_at(&s.anim_layers[0], 0.0) - 0.25).abs() < 1e-9);
        assert!(s.set_attr(&layer_ref, "weight", json!(0.5)));
        assert!((s.anim_layer_effective_weight_at(&s.anim_layers[0], 0.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn dependency_nodes_are_first_class_scene_nodes() {
        let mut s = Scene::new();
        let id = s.create_dep_node("testNode", "TestNode1");
        {
            let n = s.dep_node_mut(&id).unwrap();
            n.add_attr("input", crate::graph::AttrSpec::float(1.0));
            n.add_attr("output", crate::graph::AttrSpec::float(0.0));
        }
        assert_eq!(s.resolve("TestNode1"), Some(NodeRef::Dependency(id.clone())));
        let input = NodeRef::Dependency(id.clone());
        assert_eq!(s.get_attr(&input, "input"), Some(json!(1.0)));
        assert!(s.set_attr(&input, "input", json!(3.0)));
        assert_eq!(s.get_attr(&input, "input"), Some(json!(3.0)));
        let id2 = s.create_dep_node("testNode", "TestNode1");
        {
            let n = s.dep_node_mut(&id2).unwrap();
            n.add_attr("input", crate::graph::AttrSpec::float(1.0));
            n.add_attr("output", crate::graph::AttrSpec::float(0.0));
        }
        assert_ne!(s.dep_node(&id2).unwrap().name, s.dep_node(&id).unwrap().name, "dependency node names must be unique");
        assert!(s.deps.connect(crate::graph::PlugRef::new(&id, "input"), crate::graph::PlugRef::new(&id2, "input")));
        let input2 = NodeRef::Dependency(id2.clone());
        assert_eq!(s.get_attr(&input2, "input"), Some(json!(3.0)), "connected plug did not follow its source");
        let json = s.serialize();
        let mut loaded = Scene::new();
        loaded.load(&json).unwrap();
        assert_eq!(loaded.resolve("TestNode1"), Some(NodeRef::Dependency(id)));
        assert_eq!(loaded.get_attr(&NodeRef::Dependency(id2), "input"), Some(json!(3.0)));
    }

    #[test]
    fn flatten_layers_writes_base_animation() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        s.anim_layers = vec![
            anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 20.0), (10.0, 30.0)]),
            anim_test_layer(&mut s, "al2", &a, "translateX", false, &[(0.0, 2.0), (10.0, -3.0)]),
        ];
        let times = [0.0, 6.0, 10.0];
        let before = baked_values(&mut s, &a, "translateX", &times);
        assert!(s.flatten_anim_layers());
        assert!(s.anim_layers.is_empty());
        assert!(s.anim_layer_current.is_none());
        let after = baked_values(&mut s, &a, "translateX", &times);
        for (b, a) in before.iter().zip(after.iter()) {
            assert!((b - a).abs() < 1e-9, "flatten changed the animation: {before:?} vs {after:?}");
        }
        assert!(s.anim.curves.contains_key(&Anim::key(&a, "translateX")));
    }

    #[test]
    fn extract_selected_objects_splits_a_layer() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let b = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&b, "translateX", Some(0.0), Some(0.0));
        let mut layer = anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 4.0)]);
        let mut bc = AnimCurve::new(&b, "translateX");
        bc.set_key(0.0, 9.0, "linear");
        layer.curves.insert(Anim::key(&b, "translateX"), bc);
        layer.attrs.push(Anim::key(&b, "translateX"));
        layer.members.push(b.clone());
        s.anim_layers = vec![layer];
        let new_id = s.extract_anim_layer_objects("al1", &[a.clone()], "Layer 1_extract").expect("extract");
        assert_eq!(s.anim_layers.len(), 2);
        assert_eq!(s.anim_layers[0].id, new_id);
        assert_eq!(s.anim_layers[0].name, "Layer 1_extract");
        assert!(s.anim_layer_curve(&new_id, &Anim::key(&a, "translateX")).is_some());
        assert!(s.anim_layer_curve("al1", &Anim::key(&b, "translateX")).is_some());
        assert_eq!(s.anim_layers[1].members, vec![b]);
        assert_eq!(s.anim_layer_current.as_deref(), Some(new_id.as_str()));
    }

    #[test]
    fn layered_channels_create_maya_style_blend_and_curve_nodes() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 10.0)])];
        s.set_time(0.0);
        let blend = s.find_dep("pCube1_translateX_al1").expect("animBlendNode");
        assert_eq!(blend.kind, "animBlendNode");
        let curve = s.find_dep("pCube1_translateX_al1_inputB").expect("animCurve");
        assert_eq!(curve.kind, "animCurve");
        assert_eq!(curve.curve.as_ref().map(|c| c.keys.len()), Some(1), "keys live on the animCurve node");
        let src = crate::graph::PlugRef::new(&curve.id, "output");
        let dst = crate::graph::PlugRef::new(&blend.id, "inputB");
        assert_eq!(s.deps.source_of(&dst), Some(src), "curve output is not wired to the blend node");
        assert_eq!(s.deps.get(&dst), Some(json!(10.0)), "blend inputB does not evaluate the curve");
        assert_eq!(s.deps.get(&crate::graph::PlugRef::new(&blend.id, "output")), Some(json!(10.0)));
    }

    #[test]
    fn layer_curve_nodes_survive_save_and_load() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 10.0), (5.0, 4.0)])];
        s.set_time(2.5);
        let data = s.serialize();
        let mut t = Scene::new();
        t.load(&data).expect("load");
        let c = t.anim_layer_curve("al1", &Anim::key(&a, "translateX")).expect("curve node");
        assert_eq!(c.keys.len(), 2);
        assert!((c.eval(2.5) - 7.0).abs() < 1e-9);
        assert!(t.find_dep("pCube1_translateX_al1").is_some());
    }

    #[test]
    fn deleting_a_layer_removes_its_channel_nodes() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(0.0, 10.0)])];
        s.set_time(0.0);
        assert!(s.find_dep("pCube1_translateX_al1").is_some());
        s.remove_anim_layer_nodes("al1");
        s.remove_dep_node("al1");
        assert!(s.find_dep("pCube1_translateX_al1").is_none());
        assert!(s.find_dep("pCube1_translateX_al1_inputB").is_none());
    }

    #[test]
    fn file_round_trip_does_not_bake_layer_curves() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        for (t, v) in [(1.0, 0.0), (10.0, 5.0), (15.0, 2.0)] {
            s.set_key(&a, "translateX", Some(t), Some(v));
        }
        s.anim_layers = vec![anim_test_layer(&mut s, "al1", &a, "translateX", true, &[(1.0, 0.0), (10.0, 3.0), (15.0, 1.0)])];
        s.set_time(1.0);
        let path = std::env::temp_dir().join(format!("inca_round_trip_{}.inca", std::process::id()));
        crate::io::save_scene(&mut s, &path).expect("save");
        let mut t = Scene::new();
        crate::io::open_scene(&mut t, &path).expect("open");
        let _ = std::fs::remove_file(&path);
        let c = t.anim_layer_curve("al1", &Anim::key(&a, "translateX")).expect("layer curve");
        assert_eq!(c.keys.len(), 3, "layer curve was baked on load");
        assert_eq!(c.keys.iter().map(|k| k.t).collect::<Vec<_>>(), vec![1.0, 10.0, 15.0]);
        let base = &t.anim.curves[&Anim::key(&a, "translateX")];
        assert_eq!(base.keys.len(), 3, "base curve was baked on load");
        for time in [1.0, 5.0, 10.0, 15.0, 24.0] {
            t.set_time(time);
        }
        let c = t.anim_layer_curve("al1", &Anim::key(&a, "translateX")).expect("layer curve");
        assert_eq!(c.keys.len(), 3, "evaluating after load changed the layer keys");
        assert_eq!(t.anim.curves[&Anim::key(&a, "translateX")].keys.len(), 3, "evaluating after load changed the base keys");
    }

    #[test]
    fn animation_layers_round_trip_through_scene_json() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut c = AnimCurve::new(&a, "translateX");
        c.set_key(0.0, 2.0, "linear");
        let mut curves = IndexMap::new();
        curves.insert(Anim::key(&a, "translateX"), c);
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: CHANNELS.iter().map(|c| Anim::key(&a, c)).collect(),
            parent: None,
            curves,
        });
        s.anim_layer_current = Some("al1".into());
        s.ensure_anim_layer_nodes();
        s.set_anim_layer_bool("al1", "ghost", true);
        s.set_anim_layer_ghost_color("al1", [0.2, 0.4, 0.6]);
        s.anim_base_lock = true;
        s.anim_base_ghost = true;
        s.anim_base_ghost_color = [0.6, 0.2, 0.4];
        s.anim_solo_saved = vec![("al1".into(), false, true)];
        s.remember_anim_static(std::slice::from_ref(&a));
        let data = s.serialize();
        let mut t = Scene::new();
        t.load(&data).expect("scene loads");
        assert_eq!(t.anim_layers.len(), 1);
        assert_eq!(t.anim_layers[0].name, "Layer 1");
        assert_eq!(t.anim_layers[0].members, vec![a.clone()]);
        assert_eq!(t.anim_layer_current.as_deref(), Some("al1"));
        assert!(t.anim_layer_ghost("al1"));
        assert_eq!(t.anim_layer_ghost_color("al1"), [0.2, 0.4, 0.6]);
        assert!(t.anim_layers[0].attrs.contains(&Anim::key(&a, "translateX")));
        assert!(t.anim_base_ghost);
        assert_eq!(t.anim_base_ghost_color, [0.6, 0.2, 0.4]);
        assert!(t.anim_base_lock, "base lock survives the load");
        assert_eq!(t.anim_solo_saved, vec![("al1".into(), false, true)], "solo restore state survives the load");
        assert!((t.anim_static[&Anim::key(&a, "scaleX")] - 1.0).abs() < 1e-9, "static pose survives the load");
        let c = t.anim_layer_curve("al1", &Anim::key(&a, "translateX")).expect("curve survives");
        assert!((c.eval(0.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn animation_can_evaluate_only_the_lower_part_of_the_stack() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key(&a, "translateX", Some(0.0), Some(0.0));
        s.set_key(&a, "translateX", Some(10.0), Some(10.0));
        for (id, v) in [("top", 30.0), ("bottom", 20.0)] {
            let mut c = AnimCurve::new(&a, "translateX");
            c.set_key(0.0, v, "linear");
            c.set_key(10.0, v, "linear");
            let mut curves = IndexMap::new();
            curves.insert(Anim::key(&a, "translateX"), c);
            s.anim_layers.push(AnimLayer {
                id: id.into(),
                name: id.into(),
                weight_curve: Default::default(),
                members: vec![a.clone()],
                attrs: vec![],
                parent: None,
                curves,
            });
        }
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 55.0).abs() < 1e-9, "full stack");
        s.apply_anim_upto(Some(1));
        assert!((s.get_attr_f(&a, "translateX") - 25.0).abs() < 1e-9, "base + bottom layer");
        s.apply_anim_upto(None);
        assert!((s.get_attr_f(&a, "translateX") - 5.0).abs() < 1e-9, "BaseAnimation only");
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 55.0).abs() < 1e-9, "restored stack");
    }

    #[test]
    fn additive_layer_keys_hold_their_pose_without_compounding() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: CHANNELS.iter().map(|c| Anim::key(&a, c)).collect(),
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        // S keys every channel at the rest pose.
        s.set_time(1.0);
        for c in CHANNELS {
            s.set_key(&a, c, None, None);
        }
        // Move and key a second rotation pose.
        s.set_time(20.0);
        s.set_attr_f(&a, "rotateY", 90.0);
        s.set_key(&a, "rotateY", None, None);
        let c = layer_curve(&s, "al1", &a, "rotateY");
        assert!((c.eval(20.0) - 90.0).abs() < 1e-9, "additive key stored {}", c.eval(20.0));
        s.set_time(10.0);
        let mid = s.get_attr_f(&a, "rotateY");
        assert!(mid > 1.0 && mid < 89.0, "layer does not animate: {mid}");
        for _ in 0..200 {
            s.set_time(10.0);
        }
        assert!((s.get_attr_f(&a, "rotateY") - mid).abs() < 1e-9, "additive evaluation compounds");
        s.set_anim_layer_override("al1", true);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY") - mid).abs() < 1e-9, "override changes the pose");
        assert!(s.nodes[&a].visible, "overriding a visibility key hid the object");
    }

    #[test]
    fn animation_layer_weight_and_mute_return_to_the_static_pose() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.remember_anim_static(std::slice::from_ref(&a));
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: CHANNELS.iter().map(|c| Anim::key(&a, c)).collect(),
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        s.set_time(1.0);
        for c in CHANNELS {
            s.set_key(&a, c, None, None);
        }
        s.set_time(10.0);
        s.set_attr_f(&a, "rotateY", 80.0);
        s.set_key(&a, "rotateY", None, None);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY") - 80.0).abs() < 1e-9);
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY") - 40.0).abs() < 1e-9, "weight does not blend the pose");
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9, "weight changed the scale");
        set_layer_weight(&mut s, 0, 0.0);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY")).abs() < 1e-9, "weight 0 did not return to the static pose");
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9);
        set_layer_weight(&mut s, 0, 1.0);
        s.set_anim_layer_bool("al1", "mute", true);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY")).abs() < 1e-9, "mute did not return to the static pose");
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9);
        let keys: Vec<String> = s.anim_layers[0].curves.keys().cloned().collect();
        s.anim_layers.clear();
        s.restore_anim_static(&keys);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY")).abs() < 1e-9, "deleting the layer left the old pose");
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn override_layer_weight_does_not_scale_unkeyed_channels() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.remember_anim_static(std::slice::from_ref(&a));
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: CHANNELS.iter().map(|c| Anim::key(&a, c)).collect(),
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        s.set_time(1.0);
        for c in CHANNELS {
            s.set_key(&a, c, None, None);
        }
        s.set_time(10.0);
        s.set_attr_f(&a, "rotateY", 80.0);
        s.set_key(&a, "rotateY", None, None);
        s.set_time(10.0);
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY") - 40.0).abs() < 1e-9);
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9, "override weight scaled the object");
        set_layer_weight(&mut s, 0, 0.0);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "rotateY")).abs() < 1e-9);
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn by_layer_rotation_uses_a_quaternion_slerp() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut layer = anim_test_layer(&mut s, "al1", &a, "rotateX", true, &[(0.0, 90.0)]);
        let mut cy = AnimCurve::new(&a, "rotateY");
        cy.set_key(0.0, 90.0, "linear");
        layer.curves.insert(Anim::key(&a, "rotateY"), cy);
        layer.attrs.push(Anim::key(&a, "rotateY"));
        s.anim_layers = vec![layer];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "rotateX") - 45.0).abs() < 1e-9);
        assert!((s.get_attr_f(&a, "rotateY") - 45.0).abs() < 1e-9);
        s.set_anim_layer_rotation_by_layer("al1", true);
        s.set_time(0.0);
        let q = quat_from_euler_deg([90.0, 90.0, 0.0], "xyz");
        let want = euler_deg_from_quat_near(DQuat::IDENTITY.slerp(q, 0.5), "xyz", [0.0, 0.0, 0.0]);
        assert!((s.get_attr_f(&a, "rotateX") - want[0]).abs() < 1e-9, "by-layer X");
        assert!((s.get_attr_f(&a, "rotateY") - want[1]).abs() < 1e-9, "by-layer Y");
        assert!((s.get_attr_f(&a, "rotateZ") - want[2]).abs() < 1e-9, "by-layer Z");
    }

    #[test]
    fn flatten_bakes_by_layer_rotations() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut layer = anim_test_layer(&mut s, "al1", &a, "rotateX", true, &[(0.0, 90.0)]);
        let mut cy = AnimCurve::new(&a, "rotateY");
        cy.set_key(0.0, 90.0, "linear");
        layer.curves.insert(Anim::key(&a, "rotateY"), cy);
        layer.attrs.push(Anim::key(&a, "rotateY"));
        s.set_anim_layer_rotation_by_layer("al1", true);
        s.anim_layers = vec![layer];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        let before = [s.get_attr_f(&a, "rotateX"), s.get_attr_f(&a, "rotateY"), s.get_attr_f(&a, "rotateZ")];
        assert!(s.flatten_anim_layers());
        s.set_time(0.0);
        for (attr, want) in ["rotateX", "rotateY", "rotateZ"].iter().zip(before) {
            assert!((s.get_attr_f(&a, attr) - want).abs() < 1e-9, "{attr} changed after flatten");
        }
    }

    #[test]
    fn merging_additive_layers_keeps_visibility_on() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let target = anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 0.0)]);
        let source = anim_test_layer(&mut s, "al2", &a, "visibility", false, &[(0.0, 1.0)]);
        s.anim_layers = vec![target, source];
        s.set_time(0.0);
        assert!(s.nodes[&a].visible);
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        s.set_time(0.0);
        assert!(s.nodes[&a].visible, "merge turned visibility off");
    }

    #[test]
    fn by_layer_merge_preserves_the_target_weight() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut target = anim_test_layer(&mut s, "al1", &a, "rotateX", true, &[(0.0, 90.0)]);
        s.set_anim_layer_rotation_by_layer("al1", true);
        let mut source = anim_test_layer(&mut s, "al2", &a, "rotateY", true, &[(0.0, 90.0)]);
        s.set_anim_layer_rotation_by_layer("al2", true);
        s.anim_layers = vec![target, source];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        let before = [s.get_attr_f(&a, "rotateX"), s.get_attr_f(&a, "rotateY"), s.get_attr_f(&a, "rotateZ")];
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        assert_eq!(s.anim_layers.len(), 1);
        assert!((s.anim_layer_weight("al1") - 0.5).abs() < 1e-9, "merge changed the weight");
        s.set_time(0.0);
        for (attr, want) in ["rotateX", "rotateY", "rotateZ"].iter().zip(before) {
            let got = s.get_attr_f(&a, attr);
            assert!((got - want).abs() < 1e-9, "{attr} changed after merge: {want} -> {got}");
        }
    }

    #[test]
    fn merging_by_layer_rotations_keeps_visibility_on() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut target = anim_test_layer(&mut s, "al1", &a, "visibility", false, &[(0.0, 0.0)]);
        let mut source = anim_test_layer(&mut s, "al2", &a, "rotateX", true, &[(0.0, 90.0)]);
        s.set_anim_layer_rotation_by_layer("al2", true);
        s.anim_layers = vec![target, source];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        assert!(s.nodes[&a].visible, "visibility should be on before the merge");
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        s.set_time(0.0);
        assert!(s.nodes[&a].visible, "merge turned visibility off");
    }

    #[test]
    fn merging_by_layer_rotations_preserves_the_pose() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut target = anim_test_layer(&mut s, "al1", &a, "rotateX", true, &[(0.0, 90.0)]);
        target.weight_curve.set_key(0.0, 1.0, "linear");
        s.set_anim_layer_passthrough("al1", true);
        s.set_anim_layer_rotation_by_layer("al1", true);
        let mut source = anim_test_layer(&mut s, "al2", &a, "rotateY", true, &[(0.0, 90.0)]);
        s.set_anim_layer_rotation_by_layer("al2", true);
        s.anim_layers = vec![target, source];
        set_layer_weight(&mut s, 0, 1.0);
        s.set_time(0.0);
        let before = [s.get_attr_f(&a, "rotateX"), s.get_attr_f(&a, "rotateY"), s.get_attr_f(&a, "rotateZ")];
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        assert_eq!(s.anim_layers.len(), 1);
        assert!((s.anim_layer_weight("al1") - 1.0).abs() < 1e-9);
        assert_eq!(s.anim_layers[0].weight_curve.keys.len(), 1, "merge cleared the weight curve");
        s.set_time(0.0);
        for (attr, want) in ["rotateX", "rotateY", "rotateZ"].iter().zip(before) {
            let got = s.get_attr_f(&a, attr);
            assert!((got - want).abs() < 1e-9, "{attr} changed after merge: {want} -> {got} (before {before:?})");
        }
    }

    #[test]
    fn additive_scale_keys_store_the_ratio_to_the_layers_below() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut lower = anim_test_layer(&mut s, "al2", &a, "scaleY", false, &[(0.0, 0.0)]);
        lower.attrs = vec![Anim::key(&a, "scaleY")];
        lower.curves.clear();
        let mut upper = lower.clone();
        upper.id = "al1".into();
        upper.name = "al1".into();
        s.anim_layers = vec![upper, lower];
        s.set_time(0.0);
        s.anim_layer_current = Some("al2".into());
        s.set_attr_f(&a, "scaleY", 2.0);
        assert!(s.set_key(&a, "scaleY", Some(0.0), None));
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleY") - 2.0).abs() < 1e-9);
        s.anim_layer_current = Some("al1".into());
        s.set_attr_f(&a, "scaleY", 3.0);
        assert!(s.set_key(&a, "scaleY", Some(0.0), None));
        let c = layer_curve(&s, "al1", &a, "scaleY");
        assert!((c.keys[0].v - 1.5).abs() < 1e-9, "top additive scale should store the ratio 3/2, got {}", c.keys[0].v);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleY") - 3.0).abs() < 1e-9, "result should match Maya's 3");
    }

    #[test]
    fn additive_scale_accumulation_adds_and_keys_deltas() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut layer = anim_test_layer(&mut s, "al1", &a, "scaleX", false, &[(0.0, 0.0)]);
        s.set_anim_layer_scale_additive("al1", true);
        layer.attrs = vec![Anim::key(&a, "scaleX")];
        layer.curves.clear();
        s.anim_layers = vec![layer];
        s.anim_layer_current = Some("al1".into());
        s.set_time(0.0);
        s.set_attr_f(&a, "scaleX", 2.0);
        assert!(s.set_key(&a, "scaleX", Some(0.0), None));
        let c = layer_curve(&s, "al1", &a, "scaleX");
        assert!((c.keys[0].v - 1.0).abs() < 1e-9, "additive scale should key the delta, got {}", c.keys[0].v);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - 2.0).abs() < 1e-9);
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - 1.5).abs() < 1e-9);
    }

    #[test]
    fn merging_additive_scale_preserves_the_pose() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut target = anim_test_layer(&mut s, "al1", &a, "scaleX", false, &[(0.0, 0.0)]);
        s.set_anim_layer_scale_additive("al1", true);
        let mut source = anim_test_layer(&mut s, "al2", &a, "scaleX", false, &[(0.0, 0.5)]);
        s.set_anim_layer_scale_additive("al2", true);
        s.anim_layers = vec![target, source];
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        let before = s.get_attr_f(&a, "scaleX");
        assert!((before - 1.5).abs() < 1e-9);
        assert!(s.merge_anim_layers("al1", &["al2".into()]));
        assert!((s.anim_layer_weight("al1") - 0.5).abs() < 1e-9, "merge changed the target weight");
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - before).abs() < 1e-9);
    }

    #[test]
    fn additive_scale_multiplies_and_keeps_absolute_keys() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let layer = anim_test_layer(&mut s, "al1", &a, "scaleX", false, &[(0.0, 3.0)]);
        s.anim_layers = vec![layer];
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - 3.0).abs() < 1e-9, "additive scale should multiply the base");
        set_layer_weight(&mut s, 0, 0.5);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - 3.0_f64.sqrt()).abs() < 1e-9, "weight should be a scale exponent");
        set_layer_weight(&mut s, 0, 1.0);
        s.set_anim_layer_override("al1", true);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "scaleX") - 3.0).abs() < 1e-9, "absolute scale key should survive the mode switch");
    }

    #[test]
    fn override_reads_the_additive_offset_after_a_mode_switch() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let lower = anim_test_layer(&mut s, "al2", &a, "translateX", false, &[(0.0, 10.0)]);
        let upper = anim_test_layer(&mut s, "al1", &a, "translateX", false, &[(0.0, 0.0)]);
        s.anim_layers = vec![upper, lower];
        s.anim_layer_current = Some("al1".into());
        s.set_time(0.0);
        s.set_attr_f(&a, "translateX", 30.0);
        assert!(s.set_key(&a, "translateX", Some(0.0), None));
        assert!((s.get_attr_f(&a, "translateX") - 30.0).abs() < 1e-9);
        // The additive key stored 20 (30 - the 10 from the layer below). Switch the upper
        // layer to Override without converting: 20 becomes its absolute value, as in Maya.
        s.set_anim_layer_override("al1", true);
        s.set_time(0.0);
        assert!((s.get_attr_f(&a, "translateX") - 20.0).abs() < 1e-9);
    }

    #[test]
    fn changing_animation_layer_mode_reinterprets_its_keys() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.set_attr_f(&a, "translateX", 5.0);
        s.remember_anim_static(std::slice::from_ref(&a));
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: CHANNELS.iter().map(|c| Anim::key(&a, c)).collect(),
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        s.set_time(1.0);
        for c in CHANNELS {
            s.set_key(&a, c, None, None);
        }
        s.set_time(10.0);
        s.set_attr_f(&a, "translateX", 8.0);
        s.set_key(&a, "translateX", None, None);
        let key = Anim::key(&a, "translateX");
        let value_at = |s: &Scene, t: f64| {
            let c = layer_curve(s, "al1", &a, "translateX");
            c.keys[c.find(t).unwrap()].v
        };
        assert!((value_at(&s, 10.0) - 3.0).abs() < 1e-9, "additive key stores the offset");
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "translateX") - 8.0).abs() < 1e-9);
        s.set_anim_layer_override("al1", true);
        s.set_time(10.0);
        assert!((value_at(&s, 10.0) - 3.0).abs() < 1e-9, "Maya reinterprets, it does not convert");
        assert!((s.get_attr_f(&a, "translateX") - 3.0).abs() < 1e-9, "override should reinterpret the stored offset");
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9, "the rest-pose scale key is already absolute");
        s.set_anim_layer_override("al1", false);
        s.set_time(10.0);
        assert!((s.get_attr_f(&a, "translateX") - 8.0).abs() < 1e-9);
        assert!((s.get_attr_f(&a, "scaleX") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn base_animation_lock_rejects_new_keys() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_base_lock = true;
        s.set_key(&a, "translateX", Some(1.0), Some(5.0));
        assert!(!s.anim_is_keyed(&a, "translateX"), "locked BaseAnimation received a key");
        s.anim_base_lock = false;
        s.set_key(&a, "translateX", Some(1.0), Some(5.0));
        assert!(s.anim_is_keyed(&a, "translateX"));
    }

    #[test]
    fn parent_layer_mute_and_weight_are_inherited() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.remember_anim_static(std::slice::from_ref(&a));
        let mut c = AnimCurve::new(&a, "translateX");
        c.set_key(0.0, 10.0, "linear");
        c.set_key(10.0, 10.0, "linear");
        let mut curves = IndexMap::new();
        curves.insert(Anim::key(&a, "translateX"), c);
        s.anim_layers.push(AnimLayer {
            id: "parent".into(),
            name: "Parent".into(),
            weight_curve: Default::default(),
            members: vec![],
            attrs: vec![],
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layers.push(AnimLayer {
            id: "child".into(),
            name: "Child".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![Anim::key(&a, "translateX")],
            parent: Some("parent".into()),
            curves,
        });
        s.anim_layer_current = Some("child".into());
        s.set_anim_layer_weight("parent", 0.5);
        s.set_anim_layer_weight("child", 0.5);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 2.5).abs() < 1e-9, "weights multiply");
        if let Some(p) = s.anim_layers.iter_mut().find(|l| l.id == "parent") {
            let id = p.id.clone();
            s.set_anim_layer_bool(&id, "mute", true);
        }
        s.set_time(5.0);
        assert!(s.get_attr_f(&a, "translateX").abs() < 1e-9, "parent mute did not mute the child");
        assert!(!s.set_key(&a, "translateX", Some(6.0), Some(9.0)), "muted child received a key");
        if let Some(p) = s.anim_layers.iter_mut().find(|l| l.id == "parent") {
            let id = p.id.clone();
            s.set_anim_layer_bool(&id, "mute", false);
        }
        s.set_anim_layer_weight("parent", 1.0);
        s.set_time(5.0);
        assert!((s.get_attr_f(&a, "translateX") - 5.0).abs() < 1e-9, "parent weight did not update");
    }

    #[test]
    fn layer_keys_require_attribute_membership() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![Anim::key(&a, "translateX")],
            parent: None,
            curves: IndexMap::new(),
        });
        s.anim_layer_current = Some("al1".into());
        assert!(s.set_key(&a, "translateX", Some(1.0), Some(5.0)));
        assert!(!s.set_key(&a, "translateY", Some(1.0), Some(5.0)));
        assert!(s.anim_layer_curve("al1", &Anim::key(&a, "translateX")).is_some());
        assert!(s.anim_layer_curve("al1", &Anim::key(&a, "translateY")).is_none());
        assert!(!s.anim.curves.contains_key(&Anim::key(&a, "translateY")), "rejected key leaked to BaseAnimation");
        // an empty layer owns no attributes
        s.anim_layers[0].attrs.clear();
        s.anim_layers[0].members.clear();
        assert!(!s.set_key(&a, "translateX", Some(2.0), Some(6.0)));
    }

    #[test]
    fn old_object_membership_migrates_to_attributes() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let mut c = AnimCurve::new(&a, "translateX");
        c.set_key(0.0, 1.0, "linear");
        let mut curves = IndexMap::new();
        curves.insert(Anim::key(&a, "translateX"), c);
        s.anim_layers.push(AnimLayer {
            id: "al1".into(),
            name: "Layer 1".into(),
            weight_curve: Default::default(),
            members: vec![a.clone()],
            attrs: vec![],
            parent: None,
            curves,
        });
        let data = s.serialize();
        let mut t = Scene::new();
        t.load(&data).expect("scene loads");
        let l = &t.anim_layers[0];
        assert!(l.attrs.contains(&Anim::key(&a, "translateX")), "curve attribute was lost");
        assert!(l.attrs.contains(&Anim::key(&a, "rotateY")), "keyable attribute was not added");
        assert!(l.attrs.contains(&Anim::key(&a, "smoothLevel")), "shape attribute was not added");
        assert!(l.members.contains(&a));
    }

    #[test]
    fn component_hilite_is_separate_from_the_object_selection() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        let b = s.create_mesh("polySphere", &Params::new(), None);
        let l = s.create_locator("locator1");
        s.select(std::slice::from_ref(&a), SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Vertex));
        assert_eq!(s.hilite(), vec![a.clone()]);
        // selecting another object (outliner / MEL) does not change the hilite, as in the JS
        s.select(std::slice::from_ref(&l), SelMode::Replace);
        assert_eq!(s.sel, vec![l.clone()]);
        assert_eq!(s.hilite(), vec![a.clone()]);
        // adding another mesh to the hilite keeps both; non-mesh kinds are filtered
        s.set_hilite(&[b.clone(), l.clone()]);
        assert_eq!(s.hilite(), vec![b.clone()]);
        // leaving component mode keeps the hilited objects selected, at the end of the list
        s.select(&[l.clone(), a.clone()], SelMode::Replace);
        s.set_comp_mode(None);
        assert!(s.hilite().is_empty());
        assert_eq!(s.sel, vec![l, a, b]);
    }

    #[test]
    fn selection_only_undo_keeps_geometry() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.select(std::slice::from_ref(&id), crate::scene::SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Face));
        s.comp.insert(id.clone(), CompSel { f: [0].into_iter().collect(), ..Default::default() });
        s.checkpoint_selection("select");
        s.set_comp_mode(Some(CompMode::Edge));
        s.comp.remove(&id);
        assert_eq!(s.comp_mode, Some(CompMode::Edge));
        s.undo().unwrap();
        assert_eq!(s.comp_mode, Some(CompMode::Face));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![0]);
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().mesh.f.len(), 6);
        s.redo().unwrap();
        assert_eq!(s.comp_mode, Some(CompMode::Edge));
        assert!(s.comp_sel(&id).is_empty());
    }

    #[test]
    fn undo_keeps_component_mode_and_selection() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        s.select(std::slice::from_ref(&a), SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Vertex));
        s.comp.insert(a.clone(), CompSel { v: [0u32, 1].into_iter().collect(), ..Default::default() });
        s.checkpoint("move components");
        s.comp.insert(a.clone(), CompSel { v: [0u32, 1, 2].into_iter().collect(), ..Default::default() });
        s.undo().unwrap();
        assert_eq!(s.comp_mode, Some(CompMode::Vertex));
        assert_eq!(s.hilite(), vec![a.clone()]);
        assert_eq!(s.comp_verts(&a), vec![0, 1]);
        s.redo().unwrap();
        assert_eq!(s.comp_verts(&a), vec![0, 1, 2]);
    }

    #[test]
    fn muted_animation_is_not_evaluated() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.set_attr_f(&id, "translateX", 0.0);
        s.set_key(&id, "translateX", Some(1.0), Some(5.0));
        s.time.current = 1.0;
        s.apply_anim();
        assert!((s.get_attr_f(&id, "translateX") - 5.0).abs() < 1e-9);
        let key = Anim::key(&id, "translateX");
        s.anim.curves.get_mut(&key).unwrap().mute = true;
        s.set_attr_f(&id, "translateX", 2.0);
        s.apply_anim();
        assert!((s.get_attr_f(&id, "translateX") - 2.0).abs() < 1e-9, "muted curve drove the channel");
    }

    #[test]
    fn breakdown_and_nonkeyable_channels() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.set_key_ex(&id, "translateY", Some(3.0), Some(4.0), true);
        let c = s.anim.curve(&id, "translateY").expect("breakdown curve");
        assert!(c.find(3.0).is_some_and(|i| c.keys[i].breakdown));
        s.nodes.get_mut(&id).unwrap().nonkeyable.insert("translateZ".into());
        s.set_key(&id, "translateZ", Some(3.0), Some(1.0));
        assert!(s.anim.curve(&id, "translateZ").is_none(), "nonkeyable channel was keyed");
    }

    #[test]
    fn expressions_drive_channels() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.set_attr_f(&id, "translateX", 3.0);
        s.nodes.get_mut(&id).unwrap().expressions.insert("translateY".into(), "translateX * 2 + time".into());
        s.time.current = 4.0;
        s.apply_expressions();
        assert!((s.get_attr_f(&id, "translateY") - 10.0).abs() < 1e-9);
    }

    fn test_layer(s: &mut Scene, name: &str) -> Id {
        let id = s.new_id("l");
        let created = s.id_counter;
        s.layers.push(Layer {
            id: id.clone(),
            name: name.into(),
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
        id
    }

    #[test]
    fn layer_manager_owns_display_layer_order() {
        let mut s = Scene::new();
        let a = test_layer(&mut s, "a");
        let b = test_layer(&mut s, "b");
        let c = test_layer(&mut s, "c");
        s.ensure_display_layer_nodes();
        let order = |s: &Scene| -> Vec<String> {
            let src = crate::graph::PlugRef::new("layerManager", "displayLayers");
            s.deps.connections.iter().filter(|c| c.source == src).map(|c| c.destination.node.clone()).collect()
        };
        assert_eq!(order(&s), vec![a.clone(), b.clone(), c.clone(), "defaultLayer".to_string()]);
        s.set_display_layer_order(&[c.clone(), b.clone(), a.clone()]);
        assert_eq!(s.layers.iter().map(|l| l.id.clone()).collect::<Vec<_>>(), vec![c.clone(), b.clone(), a.clone(), "defaultLayer".to_string()]);
        assert_eq!(order(&s), vec![c.clone(), b.clone(), a.clone(), "defaultLayer".to_string()]);
        let data = s.serialize();
        let mut t = Scene::new();
        t.load(&data).expect("load");
        assert_eq!(t.layers.iter().map(|l| l.id.clone()).collect::<Vec<_>>(), vec![c.clone(), b.clone(), a.clone(), "defaultLayer".to_string()]);
        assert_eq!(order(&t), vec![c, b, a, "defaultLayer".to_string()]);
    }

    #[test]
    fn default_layer_owns_unassigned_objects_and_current_layer() {
        let mut s = Scene::new();
        let a = s.create_mesh("polyCube", &Params::new(), None);
        assert!(s.layers.iter().any(|l| l.id == "defaultLayer"));
        assert_eq!(s.current_layer.as_deref(), Some("defaultLayer"));
        s.set_display_layer_bool("defaultLayer", "visibility", false);
        s.set_display_layer_bool("defaultLayer", "shading", false);
        s.set_display_layer_bool("defaultLayer", "texturing", false);
        s.set_display_layer_bool("defaultLayer", "playback", false);
        assert!(s.is_visible(&a), "defaultLayer drawing overrides must stay locked");
        assert!(s.display_layer_visible("defaultLayer"));
        assert!(s.display_layer_shading("defaultLayer"));
        assert!(s.display_layer_texturing("defaultLayer"));
        assert!(s.display_layer_playback("defaultLayer"));
        s.set_display_layer_mode("defaultLayer", "template");
        s.set_display_layer_lod("defaultLayer", "bbox");
        s.set_display_layer_color("defaultLayer", Some(json!([1.0, 0.0, 0.0])));
        assert_eq!(s.display_layer_mode("defaultLayer"), "normal");
        assert_eq!(s.display_layer_lod("defaultLayer"), "full");
        assert!(s.display_layer_color("defaultLayer").is_none());
        let custom = test_layer(&mut s, "custom");
        s.set_current_display_layer(&custom);
        assert_eq!(s.current_layer.as_deref(), Some(custom.as_str()));
        assert_eq!(
            s.get_attr(&NodeRef::Dependency("layerManager".into()), "currentLayer"),
            Some(json!(custom.clone())),
            "current layer was not mirrored on layerManager"
        );
    }

    #[test]
    fn shading_nodes_own_attributes_and_connections() {
        let mut s = Scene::new();
        let m = s.material_named("lambert1").expect("startup lambert");
        assert!(s.is_material(&m));
        let sg = s.shading_group_of(&m).expect("startup material has a shading group");
        assert_eq!(s.deps.node(&sg).unwrap().name, "initialShadingGroup");
        assert_eq!(s.shading_group_material(&sg).as_deref(), Some(m.as_str()));
        let t = s.create_texture("checker", Some("checker1"));
        let place = s.texture(&t).and_then(|x| x.place).expect("texture has a place2dTexture");
        assert!(s.is_place(&place));
        assert_eq!(s.get_attr(&NodeRef::Material(m.clone()), "color"), Some(json!([0.5, 0.5, 0.5])));
        assert!(s.set_attr(&NodeRef::Material(m.clone()), "colorR", json!(0.25)));
        assert_eq!(s.material(&m).unwrap().attrs["color"][0], json!(0.25));
        assert!(s.set_material_map(&m, "color", &t, "outColor"));
        assert_eq!(s.material(&m).unwrap().maps["color"], json!(t));
        assert_eq!(s.deps.source_of(&crate::graph::PlugRef::new(&m, "color")), Some(crate::graph::PlugRef::new(&t, "outColor")));
        assert_eq!(s.texture(&t).unwrap().name, "checker1");
        assert_eq!(s.place(&place).unwrap().name, "place2dTexture1");
        assert!(s.clear_material_map(&m, "color"));
        assert!(s.material(&m).unwrap().maps.is_empty());
    }

    #[test]
    fn shading_nodes_round_trip_and_legacy_shading_migrates() {
        let mut s = Scene::new();
        let m = s.create_material("blinn", Some("blue"));
        let t = s.create_texture("file", Some("image"));
        assert!(s.shading_group_of(&m).is_some());
        s.set_attr(&NodeRef::Material(m.clone()), "color", json!([0.1, 0.2, 0.9]));
        s.set_attr(&NodeRef::Texture(t.clone()), "fileTextureName", json!("C:/tex.png"));
        s.set_material_map(&m, "color", &t, "outColor");
        let v = s.serialize();
        let mut back = Scene::new();
        back.load(&v).unwrap();
        assert_eq!(back.material(&m).unwrap().attrs["color"], json!([0.1, 0.2, 0.9]));
        assert_eq!(back.texture(&t).unwrap().attrs["fileTextureName"], json!("C:/tex.png"));
        assert_eq!(back.material(&m).unwrap().maps["color"], json!(t));
        assert_eq!(back.material(&m).unwrap().map_out("color"), "outColor");
        assert!(back.shading_group_of(&m).is_some());
        let sg = back.shading_group_of(&m).unwrap();
        assert_eq!(back.deps.node(&sg).unwrap().name, "blueSG");
        back.delete_material(&m);
        assert!(!back.deps.nodes.contains_key(&sg), "deleting a material must remove its shading group");

        // A pre-node file has only the shading block; loading it builds the same nodes.
        let legacy = json!({
            "nodes": [],
            "idCounter": 20,
            "shading": {
                "mats": [{"id": "m1", "name": "lambert1", "type": "lambert", "attrs": {"color": [0.2, 0.3, 0.4]}, "maps": {"color": "x2"}}],
                "texs": [{"id": "x2", "name": "file1", "type": "file", "attrs": {"fileTextureName": "legacy.png"}, "place": "p3"}],
                "places": [{"id": "p3", "name": "place2dTexture1", "attrs": {"repeatU": 2.0}}],
                "defaultMatId": "m1"
            }
        });
        let mut old = Scene::new();
        old.load(&legacy).unwrap();
        assert_eq!(old.default_mat, "m1");
        assert_eq!(old.material("m1").unwrap().attrs["color"], json!([0.2, 0.3, 0.4]));
        assert_eq!(old.texture("x2").unwrap().place.as_deref(), Some("p3"));
        assert_eq!(old.place("p3").unwrap().attrs["repeatU"], json!(2.0));
        assert_eq!(old.deps.source_of(&crate::graph::PlugRef::new("m1", "color")), Some(crate::graph::PlugRef::new("x2", "outColor")));
    }

    #[test]
    fn history_ops_are_dependency_nodes_and_round_trip() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let gen = s.history_id(&id, 0).expect("primitive history node");
        assert_eq!(s.deps.node(&gen).unwrap().kind, "polyCube");
        assert_eq!(s.history_node(&id, 0).unwrap().name, "polyCube1");
        assert_eq!(s.get_attr(&NodeRef::History(id.clone(), 0), "width"), Some(json!(1.0)));

        let mut p = Params::new();
        p.insert("faces".into(), json!([0]));
        p.insert("localTranslateZ".into(), json!(0.5));
        s.add_history(&id, "polyExtrudeFace", p);
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().history.len(), 2);
        assert_eq!(s.history_node(&id, 1).unwrap().name, "polyExtrudeFace1");
        let extrude = s.history_id(&id, 1).unwrap();
        assert_eq!(s.deps.node(&extrude).unwrap().kind, "polyExtrudeFace");

        let v = s.serialize();
        let mut back = Scene::new();
        back.load(&v).unwrap();
        assert_eq!(back.history_id(&id, 1).as_deref(), Some(extrude.as_str()));
        assert_eq!(back.deps.node(&extrude).unwrap().kind, "polyExtrudeFace");
        assert_eq!(back.get_attr(&NodeRef::History(id.clone(), 1), "localTranslateZ"), Some(json!(0.5)));

        // editing the node changes the evaluated mesh after a round trip
        let before = back.node(&id).unwrap().mesh().unwrap().mesh.v.clone();
        back.set_history_param(&id, 1, "localTranslateZ", json!(1.5));
        assert_ne!(before, back.node(&id).unwrap().mesh().unwrap().mesh.v);

        // duplicates get their own history nodes
        let dup = back.duplicate(&[id.clone()], true).into_iter().next().unwrap();
        assert_ne!(back.history_id(&dup, 1), Some(extrude));
    }

    #[test]
    fn history_parameter_edits_undo_by_checkpoint() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let base = s.node(&id).unwrap().mesh().unwrap().mesh.v.clone();
        s.checkpoint("before extrude");
        let mut p = Params::new();
        p.insert("faces".into(), json!([0]));
        p.insert("localTranslateZ".into(), json!(0.25));
        s.add_history(&id, "polyExtrudeFace", p);
        let before = s.node(&id).unwrap().mesh().unwrap().mesh.v.clone();
        s.checkpoint("before edit");
        s.set_history_param(&id, 1, "localTranslateZ", json!(2.0));
        assert_ne!(before, s.node(&id).unwrap().mesh().unwrap().mesh.v);
        assert_eq!(s.history_node(&id, 1).unwrap().params["localTranslateZ"], json!(2.0));

        assert_eq!(s.undo().as_deref(), Some("before edit"));
        assert_eq!(s.history_node(&id, 1).unwrap().params["localTranslateZ"], json!(0.25));
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().mesh.v, before, "undo did not restore the evaluated mesh");

        assert_eq!(s.undo().as_deref(), Some("before extrude"));
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().history.len(), 1, "undo did not remove the extrude node");
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().mesh.v, base, "extrude edits stayed in the mesh after the node was removed");
    }

    #[test]
    fn display_layer_state_is_stored_on_its_node() {
        let mut s = Scene::new();
        let lid = test_layer(&mut s, "layer1");
        s.ensure_display_layer_nodes();
        let n = s.deps.node(&lid).expect("displayLayer node");
        assert_eq!(n.kind, "displayLayer");
        assert_eq!(s.deps.node("layerManager").map(|n| n.kind.as_str()), Some("layerManager"));
        assert_eq!(
            s.deps.source_of(&crate::graph::PlugRef::new(&lid, "layerManager")),
            Some(crate::graph::PlugRef::new("layerManager", "displayLayers")),
            "displayLayer is not connected to layerManager"
        );
        assert_eq!(s.get_attr(&NodeRef::Dependency(lid.clone()), "visibility"), Some(json!(true)));
        s.set_display_layer_bool(&lid, "visibility", false);
        s.set_display_layer_bool(&lid, "playback", false);
        s.set_display_layer_mode(&lid, "template");
        s.set_display_layer_lod(&lid, "bbox");
        s.set_display_layer_color(&lid, Some(json!([0.2, 0.4, 0.6])));
        assert!(!s.display_layer_visible(&lid));
        assert!(!s.display_layer_playback(&lid));
        assert_eq!(s.display_layer_mode(&lid), "template");
        assert_eq!(s.display_layer_lod(&lid), "bbox");
        assert_eq!(s.display_layer_color(&lid), Some(json!([0.2, 0.4, 0.6])));
        let l = s.layers.iter().find(|l| l.id == lid).expect("layer");
        assert!(l.visible && l.playback && l.mode == "normal" && l.lod == "full" && l.color.is_none(), "legacy fields were not cleared");
    }

    #[test]
    fn legacy_display_layer_fields_migrate_into_the_node() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let lid = test_layer(&mut s, "layer1");
        s.nodes.get_mut(&id).unwrap().layer = Some(lid.clone());
        let l = s.layers.iter_mut().find(|l| l.id == lid).unwrap();
        l.visible = false;
        l.mode = "template".into();
        l.color = Some(json!([1.0, 0.0, 0.0]));
        l.playback = false;
        l.enabled = false;
        l.shading = false;
        l.texturing = false;
        l.lod = "bbox".into();
        l.anim_playback = false;
        s.ensure_display_layer_nodes();
        assert!(!s.display_layer_visible(&lid));
        assert_eq!(s.display_layer_mode(&lid), "template");
        assert_eq!(s.display_layer_color(&lid), Some(json!([1.0, 0.0, 0.0])));
        assert!(!s.display_layer_playback(&lid));
        assert!(!s.display_layer_enabled(&lid));
        assert!(!s.display_layer_shading(&lid));
        assert!(!s.display_layer_texturing(&lid));
        assert_eq!(s.display_layer_lod(&lid), "bbox");
        assert!(!s.display_layer_anim_playback(&lid));
        assert!(s.is_visible(&id), "disabled overrides must not hide the object");
    }

    #[test]
    fn display_layer_node_state_survives_load() {
        let mut s = Scene::new();
        let lid = test_layer(&mut s, "layer1");
        s.ensure_display_layer_nodes();
        s.set_display_layer_mode(&lid, "reference");
        s.set_display_layer_bool(&lid, "visibility", false);
        s.set_display_layer_color(&lid, Some(json!([0.1, 0.2, 0.3])));
        let data = s.serialize();
        let mut t = Scene::new();
        t.load(&data).expect("load");
        assert_eq!(t.display_layer_mode(&lid), "reference");
        assert!(!t.display_layer_visible(&lid));
        assert_eq!(t.display_layer_color(&lid), Some(json!([0.1, 0.2, 0.3])));
        t.layers.clear();
        t.ensure_display_layer_nodes();
        assert!(t.deps.node(&lid).is_none(), "deleted layer left its displayLayer node");
    }

    #[test]
    fn display_layer_visibility_playback_and_overrides() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        let lid = test_layer(&mut s, "layer1");
        s.nodes.get_mut(&id).unwrap().layer = Some(lid.clone());
        assert!(s.is_visible(&id));
        s.set_display_layer_bool(&lid, "visibility", false);
        assert!(!s.is_visible(&id));
        // Enable Overrides off ignores every layer setting
        s.set_display_layer_bool(&lid, "enabled", false);
        assert!(s.is_visible(&id));
        s.set_display_layer_bool(&lid, "enabled", true);
        s.set_display_layer_bool(&lid, "visibility", true);
        s.set_display_layer_bool(&lid, "playback", false);
        assert!(s.is_visible(&id), "P off hid the layer while stopped");
        s.playing = true;
        assert!(!s.is_visible(&id), "P off did not hide the layer during playback");
        s.playing = false;
        s.set_display_layer_mode(&lid, "template");
        assert!(s.is_templated_display(&id));
        assert!(s.is_template(&id));
        s.set_display_layer_bool(&lid, "enabled", false);
        assert!(!s.is_templated_display(&id));
        assert!(!s.is_template(&id));
    }

    #[test]
    fn new_objects_join_the_current_layer() {
        let mut s = Scene::new();
        let lid = test_layer(&mut s, "layer1");
        s.current_layer = Some(lid.clone());
        let before = s.create_mesh("polyCube", &Params::new(), None);
        assert!(s.node(&before).unwrap().layer.is_none(), "layer off: new object was assigned");
        s.add_new_to_current = true;
        let after = s.create_mesh("polyCube", &Params::new(), None);
        assert_eq!(s.node(&after).unwrap().layer, Some(lid));
    }
}
