//! Application state, actions (menu items / shelf buttons / hotkeys) and the frame loop.

use crate::viewport::{gpu::Gpu, Viewport};
use crate::{panels, theme, timeline, toolbar, vp_ui, windows};
use inca_core::commands::Host;
use inca_core::mel::{Interp, Val};
use inca_core::scene::{CompMode, MsgKind, Scene, SelMode};
use std::collections::{HashMap, HashSet};

/// A press shorter than this, without mouse use, is a tap; anything else is a hold.
/// Maya's default sticky hotkey time is 200 ms (`kStickyKeyTime` in `nexOpts.cpp`).
const TAP_HOLD_SECS: f64 = 0.2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Lasso,
    Paint,
    Move,
    Rotate,
    Scale,
    CvCurve,
    EpCurve,
    Arc,
    Polygon,
    Joint,
    EdgeLoop,
    MultiCut,
    TargetWeld,
    Sculpt,
    QuadDraw,
    Pencil,
    IkHandle,
    Distance,
    LatticePoint,
    PaintWeights,
}

impl Tool {
    pub fn is_manip(self) -> bool {
        matches!(self, Tool::Move | Tool::Rotate | Tool::Scale)
    }
    /// Icon of an interactive tool (tool box "last tool" button).
    pub fn icon(self) -> &'static str {
        match self {
            Tool::CvCurve => "cvCurve",
            Tool::EpCurve => "epCurve",
            Tool::Arc => "arcCurve",
            Tool::Polygon => "planar",
            Tool::Joint => "joint",
            Tool::Lasso => "lasso",
            Tool::Paint => "paintSelect",
            Tool::Move => "move",
            Tool::Rotate => "rotate",
            Tool::Scale => "scale",
            Tool::Select => "select",
            Tool::EdgeLoop => "edgeLoop",
            Tool::MultiCut => "multiCut",
            Tool::TargetWeld => "targetWeld",
            Tool::Sculpt => "softSelect",
            Tool::QuadDraw => "planar",
            Tool::Pencil => "pencilCurve",
            Tool::IkHandle => "joint",
            Tool::Distance => "locator",
            Tool::LatticePoint => "nurbsCube",
            Tool::PaintWeights => "softSelect",
        }
    }
    pub fn is_select(self) -> bool {
        matches!(self, Tool::Select | Tool::Lasso | Tool::Paint | Tool::Move | Tool::Rotate | Tool::Scale)
    }
    pub fn is_creation(self) -> bool {
        matches!(self, Tool::CvCurve | Tool::EpCurve | Tool::Arc | Tool::Polygon | Tool::Joint | Tool::Pencil | Tool::Distance)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Single,
    Four,
    TwoSide,
    TwoStack,
    /// persp on the left, top and front stacked on the right
    Three,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuSet {
    Modeling,
    Rigging,
    Animation,
    Fx,
    Rendering,
}

impl MenuSet {
    pub const ALL: [MenuSet; 5] = [MenuSet::Modeling, MenuSet::Rigging, MenuSet::Animation, MenuSet::Fx, MenuSet::Rendering];
    pub fn label(self) -> &'static str {
        match self {
            MenuSet::Modeling => "Modeling",
            MenuSet::Rigging => "Rigging",
            MenuSet::Animation => "Animation",
            MenuSet::Fx => "FX",
            MenuSet::Rendering => "Rendering",
        }
    }
}

#[derive(Default)]
pub struct Windows {
    pub script_editor: bool,
    pub hypershade: bool,
    pub graph_editor: bool,
    pub dope_sheet: bool,
    pub render_view: bool,
    pub render_settings: bool,
    pub prefs: bool,
    pub hotkeys: bool,
    pub about: bool,
    pub command_list: bool,
    pub sdk: bool,
    pub assign_material: bool,
    pub help: bool,
    pub component_editor: bool,
    pub shelf_editor: bool,
    pub hypergraph: bool,
    pub connection_editor: bool,
    pub hik: bool,
    pub rename: Option<String>,
    /// In-app notice shown after changing the rendering backend.
    pub backend_notice: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RightTab {
    ChannelBox,
    AttributeEditor,
    ToolSettings,
    ModelingToolkit,
}

/// UI-side commands raised while MEL runs (tools, windows, viewports).
#[derive(Default)]
pub struct UiHost {
    pub reqs: Vec<(String, Vec<Val>)>,
}

impl Host for UiHost {
    fn ui(&mut self, _s: &mut Scene, cmd: &str, args: &[Val]) -> Option<Result<Val, String>> {
        const UI: &[&str] = &["setToolTo", "viewFit", "FrameSelected", "FrameAll", "refresh", "play", "ScriptEditor", "HypershadeWindow", "GraphEditor", "RenderViewWindow", "OutlinerWindow", "render", "modelEditor", "workspace", "inViewMessage", "headsUpMessage", "confirmDialog", "promptDialog", "setProject"];
        if UI.contains(&cmd) {
            self.reqs.push((cmd.into(), args.to_vec()));
            return Some(Ok(Val::None));
        }
        None
    }
}

#[derive(Clone)]
struct ViewState {
    id: String,
    name: String,
    xf: inca_core::scene::Xform,
    cam: inca_core::scene::CamShape,
}

pub struct IncaApp {
    pub s: Scene,
    pub mel: Interp,
    pub tool: Tool,
    pub last_tool: Tool,
    pub layout: Layout,
    /// Multi-pane layout to restore when the maximized view is toggled back.
    pub(crate) layout_before: Layout,
    pub active_vp: usize,
    pub vps: Vec<Viewport>,
    /// Per-viewport camera history for Maya's Undo/Redo View Change (Alt+Z / Alt+Y).
    view_undo: [Vec<ViewState>; 4],
    view_redo: [Vec<ViewState>; 4],
    /// global backface culling (JS `App.backfaceCulling`), mirrored into every viewport
    pub backface: bool,
    pub menu_set: MenuSet,
    pub win: Windows,
    pub right_tab: RightTab,
    pub show_right: bool,
    pub show_outliner: bool,
    /// Material or texture selected in the Hypershade/Hypergraph. Mutually exclusive with
    /// `s.sel`, like Maya's single selection: one Attribute Editor target at a time.
    pub snode_sel: Option<String>,
    /// UI elements switched off (workspace::ELEMENTS keys)
    pub hidden: std::collections::BTreeSet<String>,
    /// what Ctrl+Space hid, to bring back
    pub ui_stash: Option<(std::collections::BTreeSet<String>, bool)>,
    pub workspace: String,
    pub cmdline: String,
    pub cmd_history: Vec<String>,
    pub help: String,
    pub last_result: String,
    pub script_text: String,
    pub script_log: Vec<(MsgKind, String)>,
    pub script_lang_rhai: bool,
    pub shelf_tab: usize,
    /// Axis orientation per manipulator (move, rotate, scale): true = world, false = object.
    /// The JS defaults are `Manip.space = { translate: 'world', rotate: 'local', scale: 'local' }`.
    pub world_space: [bool; 3],
    pub snap_grid: bool,
    pub history_on: bool,
    pub manip_size: f32,
    /// Edit Pivot: the move manipulator moves pivots instead of objects
    pub pivot_mode: bool,
    /// Key holding Edit Pivot: key, pivot mode to restore, tap toggles, press time, mouse use.
    pub pivot_hold: Option<(egui::Key, bool, bool, f64, bool)>,
    /// Key holding a snap toggle: key, action, prior state, tap toggles, press time, mouse use.
    pub snap_hold: Option<(egui::Key, String, bool, bool, f64, bool)>,
    /// Last known window focus, so hold keys are only released on an actual focus loss.
    pub was_focused: bool,
    pub gpu: Option<Gpu>,
    /// Kept so the GPU renderer can be rebuilt when MSAA changes at runtime.
    pub gpu_rs: Option<eframe::egui_wgpu::RenderState>,
    /// Last known name of each camera node, so undo/redo can retarget viewport cameras.
    camera_names: HashMap<String, String>,
    pub smooth_cache: HashMap<String, (u64, inca_core::mesh::PolyMesh)>,
    /// Per-node mesh statistics (verts, edges, faces, tris, uvs) cached by node revision.
    /// The edge count needs the whole edge topology, so `Scene::poly_count` on every frame is
    /// far too slow for heavy meshes (the JS app caches `PolyMesh.topo`).
    pub stats_cache: HashMap<String, (u64, [usize; 5])>,
    /// Per-node edge topology cached by node revision (component picking, culled wires).
    pub mesh_cache: crate::viewport::MeshCache,
    /// Last component preselection: (cursor/camera/scene key, pick).
    /// Last component preselection: (viewport, cursor/camera/scene key, pick). The JS keeps
    /// `hover`/`_ht` per viewport, so a pick from another pane must not satisfy the throttle.
    pub hover_pick: Option<(usize, u64, Option<(String, crate::viewport::Comp)>)>,
    /// When the preselection was last picked (the JS app throttles it to 30 ms).
    pub hover_at: std::time::Instant,
    pub vp_state: vp_ui::VpState,
    pub panels: panels::PanelState,
    pub windows_state: windows::WinState,
    pub playing: bool,
    pub play_clock: f64,
    pub last_action: Option<String>,
    pub clipboard: Option<serde_json::Value>,
    pub test: Option<TestRun>,
    pub recent: Vec<String>,
    /// user shelf buttons (label, MEL), saved between sessions
    pub shelves: Vec<crate::toolbar::ShelfData>,
    /// Shelf Editor: shelf and button being edited
    pub shelf_edit: (usize, Option<usize>),
    /// status line: select whole hierarchies instead of objects
    pub select_hierarchy: bool,
    pub last_comp_mode: CompMode,
    /// object types switched off in the selection masks ("mesh", "curve", ...)
    pub sel_mask_off: std::collections::HashSet<String>,
    pub lock_selection: bool,
    pub snap_curve: bool,
    pub snap_point: bool,
    pub live_surface: Option<String>,
    /// 0 off, 1 object X, 2 world X, 3 world Y, 4 world Z
    pub symmetry: usize,
    pub paint_radius: f32,
    pub play_dir: f64,
    /// Shift-drag time selection on the Time Slider
    pub range_sel: Option<(f64, f64)>,
    /// copied keys: (node, attr, keys with times relative to the copy start)
    pub key_clip: Vec<(String, String, Vec<inca_core::anim::Key>)>,
    pub prev_range: Option<(f64, f64)>,
    pub recent_projects: Vec<String>,
    pub project_win: Option<crate::project::ProjectWin>,
    /// autosave on, interval in minutes
    pub autosave: (bool, f64),
    pub last_autosave: f64,
    pub prompt: Option<crate::edit_cmds::Prompt>,
    pub dup_opts: crate::edit_cmds::DupOpts,
    pub dup_special_open: bool,
    pub add_attr: Option<crate::edit_cmds::AddAttr>,
    pub last_hidden: Vec<String>,
    pub hud: crate::edit_cmds::Hud,
    pub overlay_normals: HashSet<String>,
    pub overlay_border: HashSet<String>,
    /// measured redraw rate for the fps HUD
    pub fps_measured: f32,
    /// Last window title sent to the OS; avoid a window event every frame.
    window_title: String,
    pub mesh_tool: crate::mesh_tools::MeshToolState,
    pub rig_tool: crate::rig_tools::RigToolState,
    /// Create > Type text
    pub type_text: String,
    /// where the In-View Editor was dragged to (offset from its default corner)
    pub inview_offset: egui::Vec2,
    /// UV component mode (F12): vertex picking that selects UVs in the UV Editor
    pub uv_mode: bool,
    /// user hotkeys (Hotkey Editor)
    pub keymap: crate::hotkey_editor::Keymap,
    pub keymaps: Vec<crate::hotkey_editor::KeymapSet>,
    pub active_keymap: usize,
    /// user-defined MEL commands, bindable like actions
    pub runtime_cmds: Vec<crate::hotkey_editor::RuntimeCommand>,
    pub hk: crate::hotkey_editor::HkState,
    pub ce: crate::component_editor::CeState,
    pub prefs: crate::prefs::Prefs,
    pub hg: crate::hypergraph::HgState,
    pub hik_ui: crate::hik_ui::HikState,
    pub prefs_cat: usize,
    pub viz: crate::anim_extras::AnimViz,
    pub playblast: crate::anim_extras::PlayblastUi,
    pub mesh_opts: crate::mesh_tools::MeshToolOpts,
    pub uv_editor: crate::uv_editor::UvEditor,
    pub soft: SoftSel,
    pub marking: Option<crate::marking::Menu>,
    /// the object a marking menu was opened over
    pub marking_target: Option<String>,
    /// In-View Editor: a history node shown in the viewport after an op
    pub inview: Option<(String, usize)>,
    pub options: crate::options::Values,
    pub option_dialog: Option<String>,
    /// seconds left to show the manipulator size readout
    pub manip_size_shown: f32,
    /// (sources, duplicates) of the last Shift+D, to repeat its offset
    pub last_dup: Option<(Vec<String>, Vec<String>)>,
    /// viewport background (Alt+B cycles viewport::BG_MODES)
    pub bg_mode: usize,
    /// Space: tap toggles the layout, hold shows the hotbox
    pub space_down: Option<f64>,
    /// On Release bindings waiting for their key to come up
    pub release_keys: std::collections::HashMap<egui::Key, String>,
    pub hotbox: Option<egui::Pos2>,
    /// top-level menu currently open (main menu bar or hotbox)
    pub open_menu: Option<crate::menus::OpenMenu>,
    /// screen rect of the open menu popup (tells hotbox menu clicks from outside clicks)
    pub menu_popup_rect: Option<egui::Rect>,
}

/// Soft Selection settings (JS Manip.soft).
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SoftSel {
    pub on: bool,
    pub radius: f64,
    pub linear: bool,
}

impl Default for SoftSel {
    fn default() -> Self {
        SoftSel { on: false, radius: 2.0, linear: false }
    }
}

/// INCA_TEST / INCA_SHOT: run a MEL script at startup, save a screenshot and quit.
pub struct TestRun {
    pub script: Option<String>,
    pub shot: Option<std::path::PathBuf>,
    pub frame: u32,
    pub requested: bool,
}

impl IncaApp {
    pub fn shown(&self, element: &str) -> bool {
        !self.hidden.contains(element)
    }
    /// The manipulator tool the settings/marking menus act on (the active manip tool, or the
    /// last one when another tool is active).
    pub fn manip_tool(&self) -> Tool {
        if self.tool.is_manip() {
            self.tool
        } else {
            self.last_tool
        }
    }
    /// Axis orientation of a manipulator: true = world, false = object (JS `Manip.space`).
    pub fn space_world(&self, tool: Tool) -> bool {
        self.world_space[match tool {
            Tool::Rotate => 1,
            Tool::Scale => 2,
            _ => 0,
        }]
    }
    pub fn set_space_world(&mut self, tool: Tool, world: bool) {
        let i = match tool {
            Tool::Rotate => 1,
            Tool::Scale => 2,
            _ => 0,
        };
        self.world_space[i] = world;
    }
    pub fn set_shown(&mut self, element: &str, on: bool) {
        if on {
            self.hidden.remove(element);
        } else {
            self.hidden.insert(element.to_string());
        }
    }
    /// Backface culling is one global switch in the JS (`App.backfaceCulling`), shared by every pane.
    pub fn set_backface(&mut self, on: bool) {
        self.backface = on;
        for v in &mut self.vps {
            v.backface = on;
        }
    }
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut prefs: crate::prefs::Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        prefs.backend = crate::prefs::read_backend();
        let gpu_rs = cc.wgpu_render_state.clone();
        let gpu = gpu_rs.as_ref().map(|rs| Gpu::new(rs, prefs.msaa));
        let test = match (std::env::var("INCA_TEST").ok(), std::env::var("INCA_SHOT").ok()) {
            (None, None) => None,
            (script, shot) => Some(TestRun { script: script.and_then(|p| std::fs::read_to_string(p).ok()), shot: shot.map(Into::into), frame: 0, requested: false }),
        };
        let disk_keymaps = crate::hotkey_editor::load_keymaps_file();
        let mut keymaps: Vec<crate::hotkey_editor::KeymapSet> = disk_keymaps.as_ref().map(|f| f.sets.clone()).unwrap_or_else(|| cc.storage.and_then(|s| eframe::get_value(s, "keymaps")).unwrap_or_default());
        let mut active_keymap: usize = disk_keymaps.as_ref().map(|f| f.active).unwrap_or_else(|| cc.storage.and_then(|s| eframe::get_value(s, "activeKeymap")).unwrap_or(0));
        let legacy_keymap: crate::hotkey_editor::Keymap = cc.storage.and_then(|s| eframe::get_value(s, "keymap")).unwrap_or_default();
        if disk_keymaps.is_none() && keymaps.is_empty() && !legacy_keymap.is_empty() {
            keymaps.push(crate::hotkey_editor::KeymapSet { name: "Custom".into(), map: legacy_keymap });
            active_keymap = 1;
        }
        // Older builds stored left/right modifier keys separately; fold them into Ctrl/Shift/Alt.
        for set in &mut keymaps {
            set.map = crate::hotkey_editor::normalize_keymap(set.map.clone());
        }
        if active_keymap > keymaps.len() {
            active_keymap = 0;
        }
        let keymap: crate::hotkey_editor::Keymap = if active_keymap == 0 { Default::default() } else { keymaps[active_keymap - 1].map.clone() };
        let mut app = IncaApp {
            s: Scene::new(),
            mel: Interp::new(),
            tool: Tool::Select,
            last_tool: Tool::Move,
            layout: Layout::Single,
            layout_before: Layout::Four,
            active_vp: 0,
            vps: vec![Viewport::new("persp"), Viewport::new("top"), Viewport::new("front"), Viewport::new("side")],
        view_undo: std::array::from_fn(|_| Vec::new()),
        view_redo: std::array::from_fn(|_| Vec::new()),
            backface: false,
            menu_set: crate::prefs::default_menu_set(&prefs),
            win: Windows::default(),
            right_tab: cc.storage.and_then(|s| eframe::get_value(s, "rightTab")).unwrap_or(RightTab::ChannelBox),
            show_right: cc.storage.and_then(|s| eframe::get_value(s, "showRight")).unwrap_or(true),
            snode_sel: None,
            show_outliner: cc.storage.and_then(|s| eframe::get_value(s, "showOutliner")).unwrap_or(false),
            hidden: cc.storage.and_then(|s| eframe::get_value(s, "hiddenUi")).unwrap_or_default(),
            ui_stash: None,
            workspace: cc.storage.and_then(|s| eframe::get_value(s, "workspace")).unwrap_or_else(|| "Maya Classic".into()),
            cmdline: String::new(),
            cmd_history: vec![],
            help: String::new(),
            last_result: String::new(),
            script_text: String::from("// MEL\npolySphere -r 1;\nmove -r 0 1 0;\n"),
            script_log: vec![],
            script_lang_rhai: false,
            shelf_tab: cc.storage.and_then(|s| eframe::get_value(s, "shelfTab")).unwrap_or(1),
            world_space: [true, false, false],
            snap_grid: false,
            history_on: true,
            manip_size: cc.storage.and_then(|s| eframe::get_value(s, "manipSize")).unwrap_or(1.0),
            pivot_mode: false,
            pivot_hold: None,
            snap_hold: None,
            was_focused: true,
            gpu,
            gpu_rs,
            camera_names: HashMap::new(),
            smooth_cache: HashMap::new(),
            stats_cache: HashMap::new(),
            mesh_cache: Default::default(),
            hover_pick: None,
            hover_at: std::time::Instant::now(),
            vp_state: Default::default(),
            panels: Default::default(),
            windows_state: Default::default(),
            playing: false,
            play_clock: 0.0,
            last_action: None,
            clipboard: None,
            test,
            recent: cc.storage.and_then(|s| eframe::get_value(s, "recent")).unwrap_or_default(),
            shelves: cc.storage.and_then(|s| eframe::get_value(s, "shelves")).unwrap_or_else(|| {
                let old: Vec<(String, String)> = cc.storage.and_then(|s| eframe::get_value(s, "customShelf")).unwrap_or_default();
                crate::toolbar::default_shelves(&old)
            }),
            shelf_edit: (0, None),
            space_down: None,
            release_keys: Default::default(),
            select_hierarchy: false,
            last_comp_mode: CompMode::Vertex,
            sel_mask_off: ["template".to_string()].into_iter().collect(),
            lock_selection: false,
            snap_curve: false,
            snap_point: false,
            live_surface: None,
            symmetry: 0,
            paint_radius: 24.0,
            play_dir: 1.0,
            range_sel: None,
            key_clip: vec![],
            prev_range: None,
            recent_projects: cc.storage.and_then(|s| eframe::get_value(s, "recentProjects")).unwrap_or_default(),
            project_win: None,
            autosave: cc.storage.and_then(|s| eframe::get_value(s, "autosave")).unwrap_or((false, 10.0)),
            last_autosave: 0.0,
            prompt: None,
            dup_opts: cc.storage.and_then(|s| eframe::get_value(s, "dupOpts")).unwrap_or_default(),
            dup_special_open: false,
            add_attr: None,
            last_hidden: vec![],
            hud: cc.storage.and_then(|s| eframe::get_value(s, "hud")).unwrap_or_default(),
            overlay_normals: HashSet::new(),
            overlay_border: HashSet::new(),
            fps_measured: 0.0,
            window_title: String::new(),
            mesh_tool: Default::default(),
            rig_tool: crate::rig_tools::RigToolState::new(),
            type_text: "Inca".into(),
            inview_offset: egui::Vec2::ZERO,
            uv_mode: false,
            keymap,
            runtime_cmds: crate::hotkey_editor::load_runtime_commands_file()
                .or_else(|| cc.storage.and_then(|s| eframe::get_value(s, "runtimeCommands")))
                .unwrap_or_default(),
            keymaps,
            active_keymap,
            hk: Default::default(),
            ce: Default::default(),
            prefs,
            hg: Default::default(),
            hik_ui: Default::default(),
            prefs_cat: 0,
            viz: Default::default(),
            playblast: Default::default(),
            uv_editor: Default::default(),
            manip_size_shown: 0.0,
            marking: None,
            marking_target: None,
            inview: None,
            options: cc.storage.and_then(|s| eframe::get_value(s, "options")).unwrap_or_default(),
            option_dialog: None,
            soft: cc.storage.and_then(|s| eframe::get_value(s, "softSelect")).unwrap_or_default(),
            mesh_opts: cc.storage.and_then(|s| eframe::get_value(s, "meshTools")).unwrap_or_default(),
            last_dup: None,
            bg_mode: cc.storage.and_then(|s| eframe::get_value(s, "bgMode")).unwrap_or(0),
            hotbox: None,
            open_menu: None,
            menu_popup_rect: None,
        };
        app.panels.ol = cc.storage.and_then(|s| eframe::get_value(s, "outliner")).unwrap_or_default();
        if app.test.is_none() {
            if let Some(p) = cc.storage.and_then(|s| eframe::get_value::<String>(s, "lastProject")) {
                crate::project::set_project(&mut app, &p, true);
                app.script_log.clear();
            }
        }
        if let Some(p) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) {
            app.open_path(std::path::Path::new(&p));
        }
        app.s.modified = false;
        app.sync_camera_names();
        app
    }

    pub(crate) fn reset_camera_names(&mut self) {
        self.camera_names.clear();
        self.sync_camera_names();
    }

    /// Retarget viewport cameras after renames and undo/redo: match the camera's previous
    /// name to its current node name, so references follow the node through the change.
    fn view_state(&self, idx: usize) -> Option<ViewState> {
        let Some(id) = self.s.find(&self.vps.get(idx)?.camera) else { return None };
        let n = self.s.node(&id)?;
        let inca_core::scene::Shape::Camera(cam) = &n.shape else { return None };
        Some(ViewState { id: n.id.clone(), name: n.name.clone(), xf: n.xf.clone(), cam: cam.clone() })
    }

    /// Record the view before a navigation/frame/default-view change.
    pub fn push_view_change(&mut self, idx: usize) {
        let Some(state) = self.view_state(idx) else { return };
        let undo = &mut self.view_undo[idx.min(3)];
        undo.push(state);
        if undo.len() > 100 {
            undo.remove(0);
        }
        self.view_redo[idx.min(3)].clear();
    }

    pub fn clear_view_history(&mut self) {
        for v in &mut self.view_undo {
            v.clear();
        }
        for v in &mut self.view_redo {
            v.clear();
        }
    }

    pub fn can_undo_view(&self, idx: usize) -> bool {
        self.view_undo.get(idx.min(3)).is_some_and(|v| !v.is_empty())
    }

    pub fn can_redo_view(&self, idx: usize) -> bool {
        self.view_redo.get(idx.min(3)).is_some_and(|v| !v.is_empty())
    }

    pub fn undo_view(&mut self, idx: usize) -> bool {
        let idx = idx.min(3);
        let Some(state) = self.view_undo[idx].pop() else { return false };
        if let Some(cur) = self.view_state(idx) {
            self.view_redo[idx].push(cur);
        }
        self.apply_view_state(idx, &state);
        true
    }

    pub fn redo_view(&mut self, idx: usize) -> bool {
        let idx = idx.min(3);
        let Some(state) = self.view_redo[idx].pop() else { return false };
        if let Some(cur) = self.view_state(idx) {
            self.view_undo[idx].push(cur);
        }
        self.apply_view_state(idx, &state);
        true
    }

    fn apply_view_state(&mut self, idx: usize, state: &ViewState) {
        let id = self.s.node(&state.id).map(|n| n.id.clone()).or_else(|| self.s.find(&state.name));
        let Some(id) = id else { return };
        if let Some(n) = self.s.nodes.get_mut(&id) {
            n.xf = state.xf.clone();
            if let inca_core::scene::Shape::Camera(c) = &mut n.shape {
                *c = state.cam.clone();
            }
            self.vps[idx].camera = n.name.clone();
            self.s.rev += 1;
        }
    }

    /// Camera/view state used by the viewports. Undo restores scene nodes, but navigation is not
    /// an undoable command in Maya, so these values are reapplied after an undo/redo.
    pub(crate) fn view_cameras(&self) -> Vec<(String, inca_core::scene::Xform, inca_core::scene::CamShape)> {
        let mut out: Vec<(String, inca_core::scene::Xform, inca_core::scene::CamShape)> = vec![];
        for vp in &self.vps {
            let Some(id) = self.s.find(&vp.camera) else { continue };
            if out.iter().any(|(seen, _, _)| seen == &id) {
                continue;
            }
            let Some(n) = self.s.node(&id) else { continue };
            if let inca_core::scene::Shape::Camera(c) = &n.shape {
                out.push((n.id.clone(), n.xf.clone(), c.clone()));
            }
        }
        out
    }

    pub(crate) fn restore_view_cameras(&mut self, cameras: Vec<(String, inca_core::scene::Xform, inca_core::scene::CamShape)>) {
        let mut changed = false;
        for (id, xf, cam) in cameras {
            if let Some(n) = self.s.nodes.get_mut(&id) {
                if n.xf != xf {
                    n.xf = xf;
                    changed = true;
                }
                if let inca_core::scene::Shape::Camera(c) = &mut n.shape {
                    if *c != cam {
                        *c = cam;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.s.rev += 1;
        }
    }

    pub(crate) fn sync_camera_names(&mut self) {
        let previous = std::mem::take(&mut self.camera_names);
        let current: HashMap<String, String> = self
            .s
            .nodes
            .values()
            .filter(|n| matches!(n.shape, inca_core::scene::Shape::Camera(_)))
            .map(|n| (n.id.clone(), n.name.clone()))
            .collect();
        let retarget = |name: &str| {
            current
                .iter()
                .find(|(id, _)| previous.get(*id).is_some_and(|old| old == name))
                .map(|(_, new)| new.clone())
        };
        for vp in &mut self.vps {
            if current.values().any(|n| n == &vp.camera) {
                continue;
            }
            if let Some(new) = retarget(&vp.camera) {
                vp.camera = new;
            }
        }
        if let Some(cam) = self.s.render.get("camera").and_then(|v| v.as_str()).map(str::to_string) {
            if !current.values().any(|n| n == &cam) {
                if let Some(new) = retarget(&cam) {
                    self.s.render.insert("camera".into(), serde_json::Value::String(new));
                }
            }
        }
        self.camera_names = current;
    }

    // ------------------------------------------------------------ mesh statistics

    /// Mesh statistics of one node (verts, edges, faces, tris, uvs), cached by node revision.
    /// The edge count needs the full topology, which is too expensive to rebuild for every
    /// frame of the status bar and the HUD.
    pub fn node_stats(&mut self, id: &str) -> Option<[usize; 5]> {
        let rev = self.s.node(id)?.rev;
        if let Some((cached_rev, st)) = self.stats_cache.get(id) {
            if *cached_rev == rev {
                return Some(*st);
            }
        }
        let (rev, st) = {
            let n = self.s.node(id)?;
            let m = n.mesh()?;
            let pm = &m.mesh;
            let uvs = pm.uv.as_ref().map(|u| u.iter().flatten().map(|f| f.len()).sum()).unwrap_or(0);
            (n.rev, [pm.v.len(), pm.edge_count(), pm.f.len(), pm.tri_count(), uvs])
        };
        self.stats_cache.insert(id.to_string(), (rev, st));
        Some(st)
    }

    /// Viewport HUD poly count: totals over the visible meshes and over the picked
    /// (selected or highlighted) ones, as the JS `drawHud`.
    pub fn hud_poly_stats(&mut self) -> ([usize; 5], [usize; 5]) {
        let hil = self.s.hilite();
        let ids: Vec<(String, bool)> = self
            .s
            .all_dag(false)
            .into_iter()
            .filter(|id| self.s.is_visible(id))
            .map(|id| {
                let picked = self.s.sel.contains(&id) || hil.contains(&id);
                (id, picked)
            })
            .collect();
        let (mut tot, mut sel) = ([0usize; 5], [0usize; 5]);
        for (id, picked) in ids {
            let Some(st) = self.node_stats(&id) else { continue };
            for k in 0..5 {
                tot[k] += st[k];
                if picked {
                    sel[k] += st[k];
                }
            }
        }
        (tot, sel)
    }

    /// Total scene poly count for the status bar: the same numbers as `Scene::poly_count`,
    /// without rebuilding the edge topology every frame.
    pub fn scene_poly_count(&mut self) -> (usize, usize, usize, usize) {
        let ids: Vec<String> = self.s.nodes.keys().cloned().collect();
        let mut t = [0usize; 4];
        for id in ids {
            if let Some(st) = self.node_stats(&id) {
                for k in 0..4 {
                    t[k] += st[k];
                }
            }
        }
        (t[0], t[1], t[2], t[3])
    }

    // ------------------------------------------------------------ running commands

    /// Run MEL as one undoable step, echoing it like Maya's Script Editor.
    pub fn run_mel(&mut self, label: &str, src: &str) -> Result<Val, String> {
        self.s.checkpoint(label);
        let hist_len = |s: &Scene, id: &str| s.node(id).and_then(|n| n.mesh()).map(|m| m.history.len()).unwrap_or(0);
        let watched: Vec<(String, usize)> = self.s.hilite().into_iter().chain(self.s.sel.iter().cloned()).map(|i| { let n = hist_len(&self.s, &i); (i, n) }).collect();
        let r = self.run_mel_inner(src);
        // a new history node with channels opens the In-View Editor (JS showHistoryEditor)
        for (id, n0) in watched {
            let n1 = hist_len(&self.s, &id);
            if n1 > n0 {
                let ty = self.s.node(&id).and_then(|n| n.mesh()).map(|m| m.history[n1 - 1].ty.clone()).unwrap_or_default();
                if inca_core::ops::info(&ty).is_some_and(|i| !i.2.is_empty()) {
                    self.inview = Some((id, n1 - 1));
                }
            }
        }
        r
    }

    fn run_mel_inner(&mut self, src: &str) -> Result<Val, String> {
        let before = self.s.rev;
        let r = self.exec_mel(src, true);
        if self.s.rev == before {
            // nothing changed (a query or an error): drop the empty undo step
            self.s.undo_stack.pop();
        }
        r
    }

    /// Run MEL without an undo checkpoint (the caller made one).
    pub fn exec_mel(&mut self, src: &str, echo: bool) -> Result<Val, String> {
        if echo {
            for line in src.lines().filter(|l| !l.trim().is_empty()) {
                self.script_log.push((MsgKind::Echo, line.to_string()));
            }
        }
        let undo_like = src.trim_start().starts_with("undo") || src.trim_start().starts_with("redo");
        let cameras = undo_like.then(|| self.view_cameras());
        let mut host = UiHost::default();
        let r = self.mel.run(&mut self.s, &mut host, src);
        if let Some(cameras) = cameras {
            self.restore_view_cameras(cameras);
        }
        // Keep camera references in sync when a node is renamed.
        let name_changes = std::mem::take(&mut self.s.name_changes);
        let had_changes = !name_changes.is_empty();
        if had_changes {
            for (old, new) in name_changes {
                for vp in &mut self.vps {
                    if vp.camera == old {
                        vp.camera = new.clone();
                    }
                }
                if self.s.render.get("camera").and_then(|v| v.as_str()) == Some(old.as_str()) {
                    self.s.render.insert("camera".into(), serde_json::Value::String(new));
                }
            }
        }
        if undo_like || had_changes {
            self.sync_camera_names();
        }
        for o in self.mel.output.drain(..) {
            let _ = o;
        }
        self.flush_msgs();
        match &r {
            Ok(v) => {
                let t = v.to_string();
                if !t.is_empty() {
                    self.last_result = format!("// Result: {t} //");
                    self.script_log.push((MsgKind::Result, self.last_result.clone()));
                } else {
                    self.last_result.clear();
                }
            }
            Err(e) => {
                self.last_result = format!("// Error: {e} //");
                self.script_log.push((MsgKind::Error, self.last_result.clone()));
            }
        }
        for (cmd, args) in host.reqs {
            self.ui_command(&cmd, &args);
        }
        r
    }

    fn flush_msgs(&mut self) {
        for (k, m) in self.s.msgs.drain(..) {
            if k == MsgKind::Warning || k == MsgKind::Error {
                self.last_result = m.clone();
            }
            self.script_log.push((k, m));
        }
        if self.script_log.len() > 2000 {
            let n = self.script_log.len() - 2000;
            self.script_log.drain(..n);
        }
    }

    /// Echo a line in the Script Editor without running it (direct manipulation).
    pub fn echo(&mut self, line: impl Into<String>) {
        self.script_log.push((MsgKind::Echo, line.into()));
    }

    /// Print a warning in the Script Editor immediately. Paint-time warnings cannot wait for
    /// the next MEL command to flush `Scene::msgs`.
    pub fn warn(&mut self, msg: impl Into<String>) {
        // file/OS errors can carry line breaks; the Script Editor shows one line per message
        let msg = msg.into().split_whitespace().collect::<Vec<_>>().join(" ");
        let m = format!("// Warning: {msg}");
        self.last_result = m.clone();
        self.script_log.push((MsgKind::Warning, m));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        let m = format!("// Error: {}", msg.into());
        self.last_result = m.clone();
        self.script_log.push((MsgKind::Error, m));
    }

    fn ui_command(&mut self, cmd: &str, args: &[Val]) {
        match cmd {
            "setToolTo" => {
                let t = args.first().map(|a| a.str()).unwrap_or_default();
                self.set_tool(match t.as_str() {
                    "moveSuperContext" | "Move" => Tool::Move,
                    "RotateSuperContext" | "Rotate" => Tool::Rotate,
                    "scaleSuperContext" | "Scale" => Tool::Scale,
                    _ => Tool::Select,
                });
            }
            "viewFit" | "FrameSelected" => self.action("frameSelected"),
            "setProject" => {
                if let Some(p) = args.first().map(|a| a.str()) {
                    crate::project::set_project(self, &p, false);
                }
            }
            "FrameAll" => self.action("frameAll"),
            "ScriptEditor" => self.win.script_editor = true,
            "HypershadeWindow" => self.win.hypershade = true,
            "GraphEditor" => self.win.graph_editor = true,
            "RenderViewWindow" | "render" => {
                self.win.render_view = true;
                if cmd == "render" {
                    self.windows_state.start_render(&self.s, &self.vps[self.active_vp].camera, 0, self.prefs.smooth_divisions);
                }
            }
            "OutlinerWindow" => self.show_outliner = true,
            "play" => self.playing = !args.iter().any(|a| a.str() == "-st" || a.str() == "-state") || args.iter().any(|a| a.truthy() && a.str() != "-st"),
            _ => {}
        }
    }

    pub fn set_tool(&mut self, t: Tool) {
        if t.is_manip() {
            self.last_tool = t;
        }
        if self.tool != t {
            self.vp_state.tool_pts.clear();
            if self.tool == Tool::MultiCut {
                crate::mesh_tools::multi_cut_complete(self);
            }
            self.mesh_tool.cut.clear();
            self.mesh_tool.cut_obj = None;
            self.rig_tool.ik_start = None;
        }
        self.tool = t;
        self.help = match t {
            Tool::Select => "Select Tool: select an object",
            Tool::Lasso => "Lasso Tool: drag a freehand outline around objects or components",
            Tool::Paint => "Paint Selection Tool: drag over components (or objects) to select them; Ctrl drag to deselect",
            Tool::Move => "Move Tool: Use manipulator to move object(s). Ctrl+MMB-drag to move components along normals.",
            Tool::Rotate => "Rotate Tool: Use manipulator to rotate object(s).",
            Tool::Scale => "Scale Tool: Use manipulator to scale object(s).",
            Tool::CvCurve => "CV Curve Tool: click to place CVs, Enter to finish, Backspace removes the last one",
            Tool::EpCurve => "EP Curve Tool: click to place edit points, Enter to finish",
            Tool::Arc => "Three Point Circular Arc: click the start, a point on the arc and the end",
            Tool::Polygon => "Create Polygon Tool: click to place vertices, Enter to create the face",
            Tool::Joint => "Joint Tool: click to place joints (each new joint is parented to the last), Enter to finish the chain",
            Tool::EdgeLoop => "Insert Edge Loop Tool: click and drag on an edge to insert an edge loop and slide it. Set multiple loops in Tool Settings",
            Tool::MultiCut => "Multi-Cut Tool: click on edges/vertices to cut. Shift snaps to 10% steps, Ctrl+click inserts an edge loop. Enter or right-click completes, Backspace undoes the last point",
            Tool::TargetWeld => "Target Weld Tool: drag from a vertex onto another vertex to weld them together",
            Tool::Sculpt => "Sculpt: drag on a mesh to sculpt. Ctrl inverts, Shift smooths. Brush and size in Tool Settings",
            Tool::Pencil => "Pencil Curve Tool: drag to draw a curve freehand",
            Tool::IkHandle => "Create IK Handle: click the start joint",
            Tool::Distance => "Distance Tool: click two points to measure the distance between them",
            Tool::LatticePoint => "Lattice Point Tool: drag lattice points to deform the lattice",
            Tool::PaintWeights => "Paint Skin Weights: pick an influence in Tool Settings and paint on the skinned mesh",
            Tool::QuadDraw => "Quad Draw: click to place dots on the live surface, Shift+click to fill a quad, drag to slide, Enter to finish",
        }
        .into();
    }

    pub fn set_comp_mode(&mut self, m: Option<CompMode>) {
        self.uv_mode = false;
        if self.s.comp_mode == m {
            return;
        }
        self.s.checkpoint("selection mode");
        if let Some(c) = m {
            self.last_comp_mode = c;
        }
        let mel = match m {
            None => "selectMode -o;",
            Some(CompMode::Vertex) => "selectMode -co; selectType -pv 1;",
            Some(CompMode::Edge) => "selectMode -co; selectType -pe 1;",
            Some(CompMode::Face) => "selectMode -co; selectType -pf 1;",
        };
        let _ = self.exec_mel(mel, true);
    }

    /// Select DAG nodes by their scene IDs (not display names) and echo the equivalent MEL
    /// `select` command. The scene filters out non-selectable nodes; templates stay selectable.
    pub fn select_names(&mut self, ids: &[String], mode: SelMode) {
        debug_assert!(ids.iter().all(|i| self.s.nodes.contains_key(i)), "select_names expects node IDs, not names");
        // The viewport/outliner take over the Attribute Editor, but the Hypershade
        // Property Editor keeps the last shading node (Maya behavior).
        self.snode_sel = None;
        self.windows_state.hs.graph_sel.clear();
        let names: Vec<String> = ids.iter().map(|i| self.s.name_of(i)).collect();
        let flag = match mode {
            SelMode::Replace => "-r",
            SelMode::Add => "-add",
            SelMode::Toggle => "-tgl",
            SelMode::Remove => "-d",
        };
        if names.is_empty() {
            if mode == SelMode::Replace {
                self.echo("select -cl;");
                self.s.select(&[], SelMode::Replace);
            }
            return;
        }
        self.echo(format!("select {flag} {};", names.join(" ")));
        self.s.select(ids, mode);
    }

    pub fn open_path(&mut self, p: &std::path::Path) {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let r = if matches!(ext.as_str(), "inca" | "ma" | "mb" | "fbx") { inca_core::io::open_scene(&mut self.s, p).map(|_| vec![]) } else { inca_core::io::import(&mut self.s, p) };
        match r {
            Ok(_) => {
                self.echo(format!("file -o \"{}\";", p.display()));
                self.smooth_cache.clear();
                self.stats_cache.clear();
                self.windows_state.hs.reset_work_tabs();
                self.reset_camera_names();
                self.clear_view_history();
                self.add_recent(p);
            }
            Err(e) => self.last_result = format!("// Error: {e} //"),
        }
    }

    pub fn add_recent(&mut self, p: &std::path::Path) {
        let s = p.to_string_lossy().to_string();
        self.recent.retain(|r| r != &s);
        self.recent.insert(0, s);
        self.recent.truncate(10);
    }

    pub fn title(&self) -> String {
        let name = self.s.file.as_ref().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or("untitled".into());
        format!("{}{} - Inca{}", name, if self.s.modified { "*" } else { "" }, self.s.project.as_ref().map(|p| format!(" - {p}")).unwrap_or_default())
    }

    // ------------------------------------------------------------ actions

    /// Run a named action (menus, shelves, hotkeys all go through here).
    pub fn action(&mut self, id: &str) {
        self.open_menu = None;
        self.hotbox = None;
        if let Some(cmd) = self.runtime_cmds.iter().find(|c| c.id == id) {
            let label = cmd.label.clone();
            let script = cmd.script.clone();
            if script.trim().is_empty() {
                self.warn(format!("{label} has no MEL script."));
            } else {
                let _ = self.run_mel(&label, &script);
            }
            return;
        }
        if crate::actions::repeatable(id) {
            self.last_action = Some(id.to_string());
        }
        if let Some(mel) = crate::actions::mel_for(self, id) {
            let label = crate::actions::label(id);
            let _ = self.run_mel(&label, &mel);
            return;
        }
        crate::actions::builtin(self, id);
    }

    // ------------------------------------------------------------ frame

    /// Restore the pivot mode after a hold key. Focus loss or an early return can swallow the
    /// key-up event.
    fn end_pivot_hold(&mut self) {
        if let Some((_, was_on, _, _, _)) = self.pivot_hold.take() {
            self.pivot_mode = was_on;
        }
    }

    fn snap_state(&self, id: &str) -> bool {
        match id {
            "snapGrid" => self.snap_grid,
            "snapCurve" => self.snap_curve,
            "snapPoint" => self.snap_point,
            _ => false,
        }
    }

    fn set_snap_state(&mut self, id: &str, on: bool) {
        match id {
            "snapGrid" => self.snap_grid = on,
            "snapCurve" => self.snap_curve = on,
            "snapPoint" => self.snap_point = on,
            _ => {}
        }
    }

    fn end_snap_hold(&mut self) {
        if let Some((_, id, was_on, _, _, _)) = self.snap_hold.take() {
            self.set_snap_state(&id, was_on);
        }
    }

    fn end_holds(&mut self) {
        self.end_pivot_hold();
        self.end_snap_hold();
    }

    fn hotkeys(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if !focused && self.was_focused {
            self.end_holds();
        }
        self.was_focused = focused;
        // clicked buttons and the viewport take keyboard focus in egui; only text fields should keep it
        if let Some(id) = ctx.memory(|m| m.focused()) {
            if egui::text_edit::TextEditState::load(ctx, id).is_none() {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }
        if ctx.egui_wants_keyboard_input() || crate::uv_editor::wants_keys(self, ctx) {
            self.end_holds();
            return;
        }
        // The Hotkey Editor owns the next key press while assigning a shortcut.
        if self.win.hotkeys && self.hk.capturing {
            self.end_holds();
            return;
        }
        // A hold key is momentary when the mouse is doing something while it is down; otherwise
        // a press/release is a tap that toggles the state.
        if ctx.input(|i| i.pointer.any_down()) {
            if let Some((_, _, _, _, used)) = &mut self.pivot_hold {
                *used = true;
            }
            if let Some((_, _, _, _, _, used)) = &mut self.snap_hold {
                *used = true;
            }
        }
        let events = ctx.input(|i| i.events.clone());
        let now = ctx.input(|i| i.time);
        let scope = crate::hotkey_editor::active_scope(self, ctx);
        for e in &events {
            // Space: a tap toggles single/four view, holding it opens the hotbox
            if let egui::Event::Key { key: egui::Key::Space, pressed, repeat: false, modifiers, .. } = e {
                if modifiers.is_none() {
                    if *pressed {
                        self.space_down = Some(now);
                    } else if let Some(t0) = self.space_down.take() {
                        if now - t0 < 0.25 && self.hotbox.is_none() {
                            self.action("toggleLayout");
                        }
                    }
                }
            }
        }
        if let Some(t0) = self.space_down {
            if now - t0 >= 0.25 && self.hotbox.is_none() {
                self.hotbox = ctx.input(|i| i.pointer.hover_pos()).or(Some(ctx.content_rect().center()));
                self.open_menu = None;
            }
            ctx.request_repaint();
        }
        for e in events {
            match e {
                egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => {
                    // Edit Pivot and snapping are hold commands: tap toggles, hold is temporary.
                    if let Some((id, _press, release)) = crate::hotkey_editor::hold_binding(self, key, modifiers, scope) {
                        if release {
                            if id == "pivotEdit" {
                                if self.pivot_hold.is_none() {
                                    self.pivot_hold = Some((key, self.pivot_mode, true, now, false));
                                    self.pivot_mode = true;
                                }
                            } else if self.snap_hold.is_none() {
                                let was_on = self.snap_state(&id);
                                self.set_snap_state(&id, true);
                                self.snap_hold = Some((key, id, was_on, true, now, false));
                            }
                            continue;
                        }
                    }
                    if let Some(a) = crate::hotkey_editor::resolve(self, key, modifiers, scope) {
                        self.action(&a);
                    }
                    if let Some(a) = crate::hotkey_editor::release_action(self, key, modifiers, scope) {
                        self.release_keys.insert(key, a);
                    }
                }
                egui::Event::Key { key, pressed: false, repeat: false, .. } => {
                    if let Some((hold, was_on, tap, since, used)) = self.pivot_hold {
                        if hold == key {
                            let tap = tap && !used && now - since < TAP_HOLD_SECS;
                            self.pivot_mode = if tap { !was_on } else { was_on };
                            self.pivot_hold = None;
                            continue;
                        }
                    }
                    if let Some((hold, id, was_on, tap, since, used)) = self.snap_hold.clone() {
                        if hold == key {
                            let tap = tap && !used && now - since < TAP_HOLD_SECS;
                            let on = if tap { !was_on } else { was_on };
                            self.set_snap_state(&id, on);
                            self.snap_hold = None;
                            continue;
                        }
                    }
                    if let Some(a) = self.release_keys.remove(&key) {
                        self.action(&a);
                    }
                }
                _ => {}
            }
        }
    }

    /// Play forwards (dir 1) or backwards (-1); the same direction again stops.
    pub fn toggle_play(&mut self, dir: f64) {
        if self.playing && self.play_dir == dir {
            self.playing = false;
            let t = self.s.time.current.round();
            self.s.set_time(t);
            return;
        }
        self.playing = true;
        self.play_dir = dir;
        self.play_clock = 0.0;
    }

    fn tick_playback(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let now = ctx.input(|i| i.time);
        let fps = self.s.time.fps.max(1.0);
        if self.play_clock == 0.0 {
            self.play_clock = now;
        }
        // play every frame steps one frame per redraw; otherwise follow the clock
        let speed = self.prefs.speed().unwrap_or(1.0);
        let frames = if self.s.time.play_every { 1.0 } else { ((now - self.play_clock) * fps * speed).floor() };
        if frames >= 1.0 {
            self.play_clock = if self.s.time.play_every { now } else { self.play_clock + frames / (fps * speed) };
            let (start, end) = (self.s.time.start, self.s.time.end);
            let len = (end - start + 1.0).max(1.0);
            let mut t = self.s.time.current + frames * self.play_dir;
            let lp = self.s.time.r#loop.clone();
            if t > end {
                match lp.as_str() {
                    "once" => {
                        self.s.set_time(end);
                        self.playing = false;
                        return;
                    }
                    "oscillate" => {
                        self.play_dir = -1.0;
                        t = end - (t - end);
                    }
                    _ => t = start + (t - start).rem_euclid(len),
                }
            }
            if t < start {
                match lp.as_str() {
                    "once" => {
                        self.s.set_time(start);
                        self.playing = false;
                        return;
                    }
                    "oscillate" => {
                        self.play_dir = 1.0;
                        t = start + (start - t);
                    }
                    _ => t = end - (start - t - 1.0).rem_euclid(len),
                }
            }
            self.s.set_time(t.round());
        }
        ctx.request_repaint();
    }
}

impl eframe::App for IncaApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Test runs must not overwrite the user's real preferences and keymaps.
        if self.test.is_some() {
            return;
        }
        eframe::set_value(storage, "recent", &self.recent);
        eframe::set_value(storage, "shelves", &self.shelves);
        eframe::set_value(storage, "workspace", &self.workspace);
        eframe::set_value(storage, "bgMode", &self.bg_mode);
        eframe::set_value(storage, "outliner", &self.panels.ol);
        eframe::set_value(storage, "recentProjects", &self.recent_projects);
        eframe::set_value(storage, "autosave", &self.autosave);
        eframe::set_value(storage, "dupOpts", &self.dup_opts);
        eframe::set_value(storage, "hud", &self.hud);
        eframe::set_value(storage, "meshTools", &self.mesh_opts);
        eframe::set_value(storage, "softSelect", &self.soft);
        eframe::set_value(storage, "manipSize", &self.manip_size);
        eframe::set_value(storage, "options", &self.options);
        if self.active_keymap > 0 && self.active_keymap <= self.keymaps.len() {
            self.keymaps[self.active_keymap - 1].map = self.keymap.clone();
        }
        eframe::set_value(storage, "keymaps", &self.keymaps);
        eframe::set_value(storage, "activeKeymap", &self.active_keymap);
        eframe::set_value(storage, "keymap", &self.keymap);
        if let Err(e) = crate::hotkey_editor::save_keymaps_file(self) {
            eprintln!("Failed to save keymaps.json: {e}");
        }
        eframe::set_value(storage, "runtimeCommands", &self.runtime_cmds);
        if let Err(e) = crate::hotkey_editor::save_runtime_commands_file(&self.runtime_cmds) {
            eprintln!("Failed to save runtime_commands.json: {e}");
        }
        eframe::set_value(storage, "prefs", &self.prefs);
        if let Some(p) = &self.s.project {
            eframe::set_value(storage, "lastProject", p);
        }
        eframe::set_value(storage, "rightTab", &self.right_tab);
        eframe::set_value(storage, "shelfTab", &self.shelf_tab);
        eframe::set_value(storage, "showRight", &self.show_right);
        eframe::set_value(storage, "showOutliner", &self.show_outliner);
        // a Ctrl+Space hide isn't remembered
        let hidden = self.ui_stash.as_ref().map_or(&self.hidden, |s| &s.0);
        eframe::set_value(storage, "hiddenUi", hidden);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        crate::marking::block_pointer(self, &ctx);
        // files dropped on the window open (.inca) or import (everything else)
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if p.extension().is_some_and(|e| e == "inca" || e == "ma") && self.s.nodes.values().all(|n| n.startup()) {
                self.open_path(&p);
            } else {
                let _ = self.run_mel("import", &format!("file -i \"{}\";", p.display().to_string().replace('\\', "/")));
            }
        }
        let title = self.title();
        if self.window_title != title {
            self.window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        self.tick_playback(&ctx);
        let dt = ctx.input(|i| i.unstable_dt);
        if dt > 0.0 && dt < 0.25 {
            // ~1 s time constant, so the HUD does not jump on every frame; ignore long
            // idle gaps, otherwise one repaint after a pause would count as very low FPS.
            let alpha = 1.0 - (-dt).exp();
            let inst = 1.0 / dt;
            self.fps_measured = if self.fps_measured > 0.0 { self.fps_measured * (1.0 - alpha) + inst * alpha } else { inst };
        }
        self.hotkeys(&ctx);
        // A scene selection takes over the Attribute Editor, but the Hypershade
        // Property Editor keeps the last shading node (Maya behavior).
        if self.snode_sel.is_some() && !self.s.sel.is_empty() {
            self.snode_sel = None;
            self.windows_state.hs.graph_sel.clear();
        }
        // constraints follow their targets live (gizmo drags, channel edits)
        crate::prefs::apply(self, &ctx);
        crate::anim_extras::tick(self, &ctx);
        crate::render_view::tick(self, &ctx);
        crate::rig_tools::sync_lattice_divisions(self);
        // direct connections and constraints follow their sources live (gizmo drags, channel edits)
        let mut moved = self.s.eval_connections();
        moved.extend(self.s.eval_constraints());
        if !moved.is_empty() {
            self.s.refresh_many(&moved);
        }
        // ---- layout (Maya order: menus, status line, shelf, then the work area)
        egui::Panel::top("menubar").exact_size(22.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| crate::menus::menu_bar(self, ui));
        if self.shown("statusline") {
            egui::Panel::top("statusline").exact_size(28.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| toolbar::status_line(self, ui));
        }
        if self.shown("shelf") {
            egui::Panel::top("shelf").exact_size(64.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| toolbar::shelf(self, ui));
        }
        if self.shown("helpline") {
            egui::Panel::bottom("helpline").exact_size(20.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| timeline::help_line(self, ui));
        }
        if self.shown("cmdline") {
            egui::Panel::bottom("cmdline").exact_size(24.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| timeline::command_line(self, ui));
        }
        if self.shown("rangeslider") {
            egui::Panel::bottom("range").exact_size(26.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| timeline::range_slider(self, ui));
        }
        if self.shown("timeslider") {
            egui::Panel::bottom("timeslider").exact_size(36.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| timeline::time_slider(self, ui));
        }
        if self.shown("toolbox") {
            egui::Panel::left("toolbox").exact_size(38.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| toolbar::tool_box(self, ui));
        }
        if self.show_outliner {
            egui::Panel::left("outlinerWindow").default_size(220.0).frame(theme::panel_frame()).show(ui, |ui| panels::outliner(self, ui));
        }
        if self.shown("sidetabs") {
            egui::Panel::right("sidetabs").exact_size(20.0).resizable(false).frame(egui::Frame::NONE.fill(theme::BG2).stroke(egui::Stroke::new(1.0, theme::LINE))).show(ui, |ui| panels::side_tabs(self, ui));
        }
        if self.show_right {
            egui::Panel::right("rightpanel").default_size(300.0).min_size(220.0).frame(theme::panel_frame()).show(ui, |ui| panels::right_panel(self, ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::BG_DARK)).show(ui, |ui| vp_ui::viewports(self, ui, frame));
        windows::show(self, &ctx, frame);
        panels::ae_copies(self, &ctx);
        crate::project::project_window(self, &ctx);
        crate::edit_cmds::prompt_window(self, &ctx);
        crate::edit_cmds::dup_special_window(self, &ctx);
        crate::edit_cmds::add_attr_window(self, &ctx);
        crate::uv_editor::window(self, &ctx);
        crate::marking::show(self, &ctx);
        crate::options::dialog(self, &ctx);
        crate::project::autosave_tick(self, ctx.input(|i| i.time));
        crate::menus::hotbox(self, &ctx);
        crate::test_hook::step(self, &ctx);
        if self.panels.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
