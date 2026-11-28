//! Application state, actions (menu items / shelf buttons / hotkeys) and the frame loop.

use crate::viewport::{gpu::Gpu, Viewport};
use crate::{panels, theme, timeline, toolbar, vp_ui, windows};
use inca_core::commands::Host;
use inca_core::mel::{Interp, Val};
use inca_core::scene::{CompMode, MsgKind, Scene, SelMode};
use std::collections::{HashMap, HashSet};

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
    pub channel_control: bool,
    pub delete_attributes: bool,
    pub edit_attributes: bool,
    pub expression_editor: bool,
    pub rename: Option<String>,
    /// In-app notice shown after changing the rendering backend.
    pub backend_notice: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum RightTab {
    ChannelBox,
    AttributeEditor,
    ToolSettings,
    ModelingToolkit,
}

/// Minimum widths for the right dock. The Attribute Editor needs room for its seven menu titles
/// and the 3-column transform rows; the Channel Box is narrower.
pub(crate) const RIGHT_PANEL_MIN_W_AE: f32 = 380.0;
pub(crate) const RIGHT_PANEL_MIN_W_CB: f32 = 250.0;

pub(crate) fn right_panel_min_width(tab: RightTab) -> f32 {
    if tab == RightTab::AttributeEditor { RIGHT_PANEL_MIN_W_AE } else { RIGHT_PANEL_MIN_W_CB }
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
    /// Extrude's Show Manipulator target: the history node whose manipulator is shown.
    pub extrude_manip: Option<(String, usize)>,
    /// True when the Extrude show was explicitly recalled (Channel Box/T), which allows it in
    /// Object mode. Automatic component-mode shows have this off.
    pub extrude_forced: bool,
    /// The history node T recalls. Tool changes keep it; a new selection forgets it.
    pub manip_node: Option<(String, usize)>,
    /// Maya's Show Manipulator tool: no transform gizmo for the selection until W/E/R/Q.
    pub show_manips_tool: bool,
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
    /// Main panels currently undocked into floating windows.
    pub detached: HashSet<&'static str>,
    /// Where each panel was dragged out, and a generation for a fresh floating window.
    pub detach_pos: HashMap<&'static str, egui::Pos2>,
    pub detach_gen: HashMap<&'static str, u64>,
    /// Panel whose floating window should follow the pointer until release.
    pub drag_panel: Option<&'static str>,

    /// Material or texture selected in the Hypershade/Hypergraph. Mutually exclusive with
    /// `s.sel`, like Maya's single selection: one Attribute Editor target at a time.
    pub snode_sel: Option<String>,
    /// Dependency node selected in the Outliner (animLayer, shading utility...). Mutually
    /// exclusive with `s.sel`, so one panel shows one selected node at a time.
    pub dep_sel: Option<String>,
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
    /// Edit Pivot options and the custom pivot orientation (Maya's D-mode marking menu).
    pub pivot_edit: crate::vp_ui::PivotEdit,
    /// Key holding Edit Pivot: key, the pivot mode to restore on release, press time, and
    /// whether both press and release are bound (tap toggles, hold is temporary).
    pub pivot_hold: Option<(egui::Key, bool, f64, bool)>,
    /// Whether something was selected last frame (Edit Pivot ends when the selection is cleared).
    pub pivot_had_selection: bool,
    /// Key holding a snap toggle: key, action, and the state to restore on release.
    pub snap_hold: Option<(egui::Key, String, bool)>,
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
    /// Screen rect of the floating In-View Editor, so viewport input can ignore clicks on it.
    pub inview_rect: Option<egui::Rect>,
    /// A primary press started on the In-View Editor; ignore the viewport until it is released.
    pub inview_press: bool,
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

    pub fn detach_panel(&mut self, panel: &'static str, pos: Option<egui::Pos2>) {
        self.detached.insert(panel);
        if let Some(p) = pos {
            self.detach_pos.insert(panel, p);
        }
        *self.detach_gen.entry(panel).or_default() += 1;
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
    /// Maya leaves Edit Pivot when the selection is cleared (a click on empty space, Deselect
    /// All, ...). A held D stays down but no longer turns the mode back on when released.
    pub fn end_pivot_mode_on_deselect(&mut self) {
        let has = !self.s.sel.is_empty();
        if self.pivot_mode && self.pivot_had_selection && !has {
            self.pivot_mode = false;
            if let Some(h) = self.pivot_hold.as_mut() {
                h.1 = false;
            }
        }
        self.pivot_had_selection = has;
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
            extrude_manip: None,
            extrude_forced: false,
            manip_node: None,
            show_manips_tool: false,
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
            dep_sel: None,
            show_outliner: cc.storage.and_then(|s| eframe::get_value(s, "showOutliner")).unwrap_or(false),
            detached: Default::default(),
            detach_pos: Default::default(),
            detach_gen: Default::default(),
            drag_panel: None,
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
            pivot_edit: Default::default(),
            pivot_hold: None,
            pivot_had_selection: false,
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
            inview_rect: None,
            inview_press: false,
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
        if let Some(saved) = cc.storage.and_then(|s| eframe::get_value::<Vec<(u64, [f32; 4])>>(s, "windowRects")) {
            app.windows_state.window_last_rect = crate::windows::decode_window_rects(saved);
        }
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
                if let Some(h) = self.s.history_meta(&id, n1 - 1) {
                    if inca_core::ops::info(&h.ty).is_some_and(|i| !i.2.is_empty()) {
                        self.inview = Some((id.clone(), n1 - 1));
                        // T recalls the manipulator of the operation just run, until a new selection.
                        self.manip_node = Some((id.clone(), n1 - 1));
                        // The new operation's gizmo takes over from any Channel Box channel manip.
                        self.panels.cb_armed = false;
                        // Maya opens the new node's Channel Box INPUTS entry.
                        self.panels.expanded.insert(format!("{id}:{}", h.name));
                    }
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

    /// Close the Extrude Show Manipulator and the In-View Editor belonging to it.
    pub fn end_extrude(&mut self) {
        self.extrude_forced = false;
        if let Some(t) = self.extrude_manip.take() {
            self.collapse_history_entry(&t.0, t.1);
            if self.inview.as_ref() == Some(&t) {
                self.inview = None;
            }
        }
    }

    /// T: Maya's Show Manipulator Tool. It only recalls the remembered node; any viewport click
    /// has already dropped that, so T cannot revive the Extrude after a selection.
    pub fn show_manipulator(&mut self) {
        let node = self.manip_node.clone();
        // Show Manipulator supersedes the Channel Box virtual-slider manipulator; clicking a
        // channel name re-arms that, and T disarms it again.
        self.panels.cb_armed = false;
        // Leave a creation/transform tool if Show Manipulator is not already the tool. Staying
        // in Show Manipulator must not end the current show (that would collapse its entry).
        if !self.show_manips_tool {
            self.set_tool(Tool::Select);
            self.show_manips_tool = true;
        }
        let Some((mesh, idx)) = node else {
            self.extrude_manip = None;
            self.extrude_forced = false;
            self.inview = None;
            return;
        };
        let Some(ty) = self.s.history_meta(&mesh, idx).map(|h| h.ty) else {
            self.manip_node = None;
            self.extrude_manip = None;
            self.extrude_forced = false;
            self.inview = None;
            return;
        };
        let editable = ty == "polyExtrudeFace" || inca_core::ops::info(&ty).is_some_and(|i| !i.2.is_empty());
        if !editable {
            self.manip_node = None;
            self.extrude_manip = None;
            self.extrude_forced = false;
            self.inview = None;
            return;
        }
        self.extrude_manip = (ty == "polyExtrudeFace").then(|| (mesh.clone(), idx));
        self.extrude_forced = self.extrude_manip.is_some();
        self.manip_node = Some((mesh.clone(), idx));
        self.inview = Some((mesh.clone(), idx));
        // T re-expands the node's Channel Box INPUTS entry along with its editor/gizmo.
        if let Some(name) = self.s.history_meta(&mesh, idx).map(|h| h.name) {
            self.panels.expanded.insert(format!("{mesh}:{name}"));
        }
    }

    /// Explicitly close the current In-View Editor/Show Manipulator (its own close button).
    pub fn close_manip(&mut self) {
        self.extrude_forced = false;
        if let Some(t) = self.inview.take() {
            self.collapse_history_entry(&t.0, t.1);
            if self.extrude_manip.as_ref() == Some(&t) {
                self.extrude_manip = None;
            }
            if self.manip_node.as_ref() == Some(&t) {
                self.manip_node = None;
            }
        }
    }

    /// A component-mode selection (or a deselect) drops the current In-View Editor/Show
    /// Manipulator; only the Channel Box history entry or T can re-arm it.
    pub fn disarm_manip(&mut self) {
        self.extrude_forced = false;
        if let Some((mesh, idx)) = self.extrude_manip.take() {
            self.collapse_history_entry(&mesh, idx);
        }
        if let Some((mesh, idx)) = self.inview.take() {
            self.collapse_history_entry(&mesh, idx);
        }
        self.manip_node = None;
    }

    /// Collapse the Channel Box INPUTS entry for the armed Extrude node without dropping the show.
    pub fn collapse_extrude_entry(&mut self) {
        if let Some((mesh, idx)) = self.extrude_manip.clone() {
            self.collapse_history_entry(&mesh, idx);
        }
    }

    /// Collapse the Channel Box INPUTS entry for a history node.
    fn collapse_history_entry(&mut self, mesh: &str, idx: usize) {
        if let Some(name) = self.s.history_meta(mesh, idx).map(|h| h.name) {
            self.panels.expanded.remove(&format!("{mesh}:{name}"));
        }
    }

    pub fn set_tool(&mut self, t: Tool) {
        // Maya's Multi-Cut switches into component mode; without a dedicated Multi selection
        // mode, use the last component mode and let the tool highlight the closest edge itself.
        if t == Tool::MultiCut && self.s.comp_mode.is_none() {
            self.set_comp_mode(Some(self.last_comp_mode));
        }
        // Choosing any tool leaves Show Manipulator: close its editor and extrude session.
        if self.show_manips_tool {
            self.inview = None;
        }
        self.end_extrude();
        self.show_manips_tool = false;
        if t.is_manip() {
            self.last_tool = t;
        }
        if self.tool != t {
            self.vp_state.tool_pts.clear();
            if self.tool == Tool::MultiCut {
                crate::mesh_tools::multi_cut_complete(self);
            }
            self.mesh_tool.clear_cut();
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
            Tool::MultiCut => "Multi-Cut Tool: click on edges/vertices to cut. Shift snaps to 10% steps, middle-click cuts an edge at its midpoint, Ctrl+click inserts an edge loop (Ctrl+middle-click: through the midpoint). Enter or right-click completes, Backspace/Delete undoes the last point",
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
        // Entering Object mode hides an automatic Extrude show but does not drop it: the node
        // stays armed and the Channel Box INPUTS entry stays open. Switching back to component
        // mode shows it again.
        if m.is_none() {
            self.extrude_forced = false;
        }
        if self.show_manips_tool
            && self
                .inview
                .as_ref()
                .is_some_and(|(id, i)| self.s.history_meta(id, *i).is_none_or(|h| h.ty != "polyExtrudeFace"))
        {
            self.inview = None;
        }
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
        self.dep_sel = None;
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
        let panel = match id {
            "panelShelf" => Some("shelf"),
            "panelToolbox" => Some("toolbox"),
            "panelOutliner" => Some("outliner"),
            "panelRight" => Some("rightpanel"),
            "panelTimeslider" => Some("timeslider"),
            "panelRangeslider" => Some("rangeslider"),
            "panelCmdline" => Some("cmdline"),
            _ => None,
        };
        if let Some(panel) = panel {
            if !self.detached.remove(panel) {
                self.detach_panel(panel, None);
            }
            return;
        }
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
            let ok = self.run_mel(&label, &mel).is_ok();
            if id == "extrude" && ok {
                // Maya's performPolyExtrude ends with `setToolTo ShowManips`: the new node's
                // manipulator stays live until another tool or mode is chosen.
                let target = self.inview.clone().filter(|(m, i)| {
                    self.s.history_meta(m, *i).is_some_and(|h| h.ty == "polyExtrudeFace")
                });
                // Maya switches to the Show Manipulator tool here; it selects like the Select
                // tool and shows no transform gizmo for the selection.
                self.set_tool(Tool::Select);
                self.show_manips_tool = true;
                self.inview = target.clone();
                self.extrude_manip = target.clone();
                self.manip_node = target;
                self.extrude_forced = false;
            }
            return;
        }
        crate::actions::builtin(self, id);
    }

    // ------------------------------------------------------------ frame

    /// Restore the pivot mode after a hold key. Focus loss or an early return can swallow the
    /// key-up event.
    fn end_pivot_hold(&mut self) {
        if let Some((_, was_on, _, _)) = self.pivot_hold.take() {
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
        if let Some((_, id, was_on)) = self.snap_hold.take() {
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
        if self.windows_state.key_clipboard_marker {
            ctx.copy_text("Inca clipboard".to_string());
            self.windows_state.key_clipboard_marker = false;
        }
        // clicked buttons and the viewport take keyboard focus in egui; only text fields should keep it
        if let Some(id) = ctx.memory(|m| m.focused()) {
            if egui::text_edit::TextEditState::load(ctx, id).is_none() {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        }
        // The Hotkey Editor owns the next key press while assigning a shortcut.
        let scope = crate::hotkey_editor::active_scope(self, ctx);
        // egui-winit turns Ctrl+C/Ctrl+V into clipboard events rather than key events.
        let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy) || matches!(e, egui::Event::Key { key: egui::Key::C, pressed: true, repeat: false, modifiers, .. } if modifiers.ctrl || modifiers.command)));
        let paste = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: egui::Key::V, pressed: true, repeat: false, modifiers, .. } if modifiers.ctrl || modifiers.command)));
        let editor_scope = matches!(scope, crate::hotkey_editor::Scope::GraphEditor | crate::hotkey_editor::Scope::Timeline);
        if (copy || paste) && (editor_scope || !ctx.egui_wants_keyboard_input()) {
            self.hk.capturing = false;
            self.hk.pending = None;
            if copy {
                match scope {
                    crate::hotkey_editor::Scope::GraphEditor => self.action("graphCopy"),
                    crate::hotkey_editor::Scope::Timeline => self.action("timelineCopy"),
                    _ => self.action("copy"),
                }
            }
            if paste {
                match self.windows_state.key_clip_kind {
                    crate::windows::KeyClipKind::Graph if !self.windows_state.ge_clipboard.is_empty() => self.action("graphPaste"),
                    crate::windows::KeyClipKind::Timeline if !self.key_clip.is_empty() => self.action("timelinePaste"),
                    _ => self.action("paste"),
                }
            }
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Copy | egui::Event::Paste(_)) && !matches!(e, egui::Event::Key { key: egui::Key::C | egui::Key::V, pressed: true, repeat: false, modifiers, .. } if modifiers.ctrl || modifiers.command)));
        }
        if self.win.hotkeys && self.hk.capturing {
            self.end_holds();
            return;
        }
        if ctx.egui_wants_keyboard_input() || crate::uv_editor::wants_keys(self, ctx) {
            self.end_holds();
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        let now = ctx.input(|i| i.time);
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
                    if let Some((id, press, release)) = crate::hotkey_editor::hold_binding(self, key, modifiers, scope) {
                        if release {
                            if id == "pivotEdit" {
                                if self.pivot_hold.is_none() {
                                    self.pivot_hold = Some((key, self.pivot_mode, now, press));
                                    self.pivot_mode = true;
                                }
                            } else if self.snap_hold.is_none() {
                                let was_on = self.snap_state(&id);
                                self.set_snap_state(&id, true);
                                self.snap_hold = Some((key, id, was_on));
                            }
                            continue;
                        }
                    }
                    if let Some(a) = crate::hotkey_editor::resolve(self, key, modifiers, scope) {
                        // Maya deletes the highlighted edge/face with Delete/Backspace when no
                        // component is selected; otherwise Delete acts on the selection.
                        if a == "deleteSel" && crate::vp_ui::delete_hovered_component(self, ctx) {
                            // handled
                        } else {
                            self.action(&a);
                        }
                    }
                    if let Some(a) = crate::hotkey_editor::release_action(self, key, modifiers, scope) {
                        self.release_keys.insert(key, a);
                    }
                }
                egui::Event::Key { key, pressed: false, repeat: false, .. } => {
                    if let Some((hold, was_on, t0, both)) = self.pivot_hold {
                        if hold == key {
                            let tap = both && now - t0 < 0.25;
                            self.pivot_mode = if tap { !was_on } else { was_on };
                            self.pivot_hold = None;
                            continue;
                        }
                    }
                    if let Some((hold, id, was_on)) = self.snap_hold.clone() {
                        if hold == key {
                            self.set_snap_state(&id, was_on);
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

/// Maya's double-dotted grip on a horizontal bar: drag it out to undock.
fn panel_grip_left(app: &mut IncaApp, ui: &mut egui::Ui, panel: &'static str, content: egui::Rect) {
    let rect = egui::Rect::from_min_size(content.left_top() - egui::vec2(3.0, 0.0), egui::vec2(3.0, content.height()));
    let resp = ui.interact(rect, ui.id().with(("grip_left", panel)), egui::Sense::drag());
    let painter = ui.painter().with_clip_rect(ui.clip_rect().union(rect));
    let color = theme::TEXT_DIM;
    let mut y = rect.top() + 2.0;
    while y < rect.bottom() - 1.0 {
        painter.circle_filled(egui::pos2(rect.center().x - 0.5, y), 0.6, color);
        painter.circle_filled(egui::pos2(rect.center().x + 0.5, y), 0.6, color);
        y += 4.0;
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::Grab);
    if resp.dragged() && resp.drag_delta().length() > 10.0 {
        let pos = ui.ctx().pointer_interact_pos().or_else(|| ui.ctx().input(|i| i.pointer.hover_pos()));
        app.detach_panel(panel, pos);
        app.drag_panel = Some(panel);
    }
}

/// The grip on a vertical panel.
fn panel_grip_top(app: &mut IncaApp, ui: &mut egui::Ui, panel: &'static str) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), egui::Sense::drag());
    let color = theme::TEXT_DIM;
    let mut x = rect.left() + 3.0;
    while x < rect.right() - 2.0 {
        ui.painter().circle_filled(egui::pos2(x, rect.center().y - 1.0), 0.7, color);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y + 1.0), 0.7, color);
        x += 4.0;
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::Grab);
    if resp.dragged() && resp.drag_delta().length() > 10.0 {
        let pos = ui.ctx().pointer_interact_pos().or_else(|| ui.ctx().input(|i| i.pointer.hover_pos()));
        app.detach_panel(panel, pos);
        app.drag_panel = Some(panel);
    }
}

impl eframe::App for IncaApp {
    fn persist_egui_memory(&self) -> bool {
        // Test runs must not overwrite the user's real window layout.
        self.test.is_none()
    }

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
        eframe::set_value(storage, "windowRects", &crate::windows::encode_window_rects(&self.windows_state.window_last_rect));
        // a Ctrl+Space hide isn't remembered
        let hidden = self.ui_stash.as_ref().map_or(&self.hidden, |s| &s.0);
        eframe::set_value(storage, "hiddenUi", hidden);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.end_pivot_mode_on_deselect();
        self.windows_state.window_seen_now.clear();
        self.windows_state.timeline_rect = None;
        crate::marking::block_pointer(self, &ctx);
        // Clicking anywhere outside a numeric Channel Box / Attribute Editor value field drops
        // that multi-field selection. The field rects come from the previous frame.
        if ctx.input(|i| i.pointer.primary_pressed()) {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                if !self.panels.cb_numeric_rects.iter().any(|r| r.contains(p)) {
                    crate::panels::commit_cb_edit(self);
                    self.panels.cb_fields.clear();
                    if let Some(id) = ctx.memory(|m| m.focused()) {
                        ctx.memory_mut(|m| m.surrender_focus(id));
                    }
                }
            }
        }
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
        // Layer `P` flags hide their objects only while the timeline is playing.
        self.s.playing = self.playing;
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
        if self.dep_sel.is_some() && (!self.s.sel.is_empty() || self.dep_sel.as_ref().is_some_and(|id| self.s.deps.node(id).is_none())) {
            self.dep_sel = None;
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
        if self.shown("shelf") && !self.detached.contains("shelf") {
            egui::Panel::top("shelf").exact_size(64.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| {
                let rect = ui.max_rect();
                egui::Frame::NONE.inner_margin(egui::Margin { left: 4, right: 0, top: 0, bottom: 0 }).show(ui, |ui| toolbar::shelf(self, ui));
                panel_grip_left(self, ui, "shelf", rect);
            });
        }
        if self.shown("helpline") {
            egui::Panel::bottom("helpline").exact_size(20.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| timeline::help_line(self, ui));
        }
        if self.shown("cmdline") && !self.detached.contains("cmdline") {
            egui::Panel::bottom("cmdline").exact_size(24.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| {
                let rect = ui.max_rect();
                egui::Frame::NONE.inner_margin(egui::Margin { left: 4, right: 0, top: 0, bottom: 0 }).show(ui, |ui| timeline::command_line(self, ui));
                panel_grip_left(self, ui, "cmdline", rect);
            });
        }
        if self.shown("rangeslider") && !self.detached.contains("rangeslider") {
            egui::Panel::bottom("range").exact_size(26.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| {
                let rect = ui.max_rect();
                self.windows_state.timeline_rect = Some(self.windows_state.timeline_rect.map_or(rect, |old| old.union(rect)));
                egui::Frame::NONE.inner_margin(egui::Margin { left: 4, right: 0, top: 0, bottom: 0 }).show(ui, |ui| timeline::range_slider(self, ui));
                panel_grip_left(self, ui, "rangeslider", rect);
            });
        }
        if self.shown("timeslider") && !self.detached.contains("timeslider") {
            egui::Panel::bottom("timeslider").exact_size(36.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| {
                let rect = ui.max_rect();
                self.windows_state.timeline_rect = Some(self.windows_state.timeline_rect.map_or(rect, |old| old.union(rect)));
                egui::Frame::NONE.inner_margin(egui::Margin { left: 4, right: 0, top: 0, bottom: 0 }).show(ui, |ui| timeline::time_slider(self, ui));
                panel_grip_left(self, ui, "timeslider", rect);
            });
        }
        if self.shown("toolbox") && !self.detached.contains("toolbox") {
            egui::Panel::left("toolbox").exact_size(38.0).resizable(false).frame(theme::bar_frame()).show(ui, |ui| {
                panel_grip_top(self, ui, "toolbox");
                toolbar::tool_box(self, ui);
            });
        }
        if self.show_outliner && !self.detached.contains("outliner") {
            egui::Panel::left("outlinerWindow").default_size(220.0).frame(theme::panel_frame()).show(ui, |ui| {
                panel_grip_top(self, ui, "outliner");
                panels::outliner(self, ui);
            });
        }
        if self.shown("sidetabs") {
            egui::Panel::right("sidetabs").exact_size(20.0).resizable(false).frame(egui::Frame::NONE.fill(theme::BG2).stroke(egui::Stroke::new(1.0, theme::LINE))).show(ui, |ui| panels::side_tabs(self, ui));
        }
        if self.show_right && !self.detached.contains("rightpanel") {
            let min_w = right_panel_min_width(self.right_tab);
            // Separate panel IDs give every right-side tab its own remembered width.
            let panel_id = egui::Id::new(("rightpanel", self.right_tab));
            egui::Panel::right(panel_id).default_size(min_w).min_size(min_w).frame(theme::panel_frame()).show(ui, |ui| {
                panel_grip_top(self, ui, "rightpanel");
                panels::right_panel(self, ui);
            });
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
        // Multi-Cut uses Maya's knife cursor over the 3D view. Set it after every window and
        // menu so a floating panel on top of the viewport keeps its own cursor.
        let cursor = match self.tool {
            Tool::MultiCut if self.mesh_tool.move_drag.is_some() || self.mesh_tool.hover_point.is_some() => crate::icons::move_point_cursor(),
            Tool::MultiCut => crate::icons::multicut_cursor(),
            _ => None,
        };
        let pointer = ctx.pointer_hover_pos();
        let over_viewport = pointer.is_some_and(|p| self.vps.iter().any(|v| v.rect.contains(p)));
        let top_layer = pointer.and_then(|p| ctx.layer_id_at(p));
        let cursor = (over_viewport && top_layer.is_some_and(|l| l == egui::LayerId::background())).then_some(cursor).flatten();
        ctx.set_cursor_image(cursor);
        std::mem::swap(&mut self.windows_state.window_seen, &mut self.windows_state.window_seen_now);
        if self.panels.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: egui::Key, ctrl: bool, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers { ctrl, ..Default::default() },
        }
    }

    fn raw_with(events: Vec<egui::Event>) -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
        raw.events = events;
        raw
    }

    #[test]
    fn t_recalls_the_extrude_manipulator_after_a_tool_change() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.comp_mode = Some(CompMode::Face);
        app.s.comp.entry(id.clone()).or_default().f.insert(0);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.s.set_hilite(std::slice::from_ref(&id));
        app.action("extrude");
        let extrude = app.extrude_manip.clone().expect("extrude manipulator");
        assert_eq!(app.manip_node, Some(extrude.clone()));
        // W leaves Show Manipulator; T recalls the current node.
        app.set_tool(Tool::Move);
        assert!(app.extrude_manip.is_none());
        assert_eq!(app.manip_node, Some(extrude.clone()));
        assert_eq!(
            crate::hotkey_editor::resolve(&app, egui::Key::T, egui::Modifiers::default(), crate::hotkey_editor::Scope::Application),
            Some("manipulatorTool".to_string())
        );
        app.show_manipulator();
        assert_eq!(app.extrude_manip, Some(extrude));
        assert!(app.inview.is_some());
        assert!(app.show_manips_tool);
        // Object mode: the show stays armed and T recalls it.
        app.set_comp_mode(None);
        app.show_manipulator();
        assert!(app.extrude_manip.is_some(), "T must recall Extrude in Object mode");
        assert!(app.extrude_forced);
        // After a component-mode click the tool is dropped; T must not revive it.
        app.disarm_manip();
        app.show_manipulator();
        assert!(app.extrude_manip.is_none(), "T must not recall Extrude after a click");
        assert!(app.inview.is_none());
    }

    #[test]
    fn t_recalls_other_history_nodes_and_expands_their_entry() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        let mut p = inca_core::params::Params::new();
        p.insert("fraction".into(), serde_json::json!(0.2));
        app.s.add_history(&id, "polyBevel", p);
        app.s.select(std::slice::from_ref(&id), SelMode::Replace);
        app.manip_node = Some((id.clone(), 1));
        app.show_manips_tool = false;
        app.panels.expanded.clear();
        app.show_manipulator();
        assert_eq!(app.inview, Some((id.clone(), 1)), "T must open the node's In-View Editor");
        assert!(app.extrude_manip.is_none(), "a non-Extrude node has no viewport gizmo");
        assert!(app.show_manips_tool);
        let hname = app.s.history_meta(&id, 1).expect("history node").name;
        assert!(app.panels.expanded.contains(&format!("{id}:{hname}")), "T must re-expand the Channel Box entry");
    }

    #[test]
    fn object_mode_remembers_the_previous_component_mode() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = IncaApp::new(&cc);
        app.set_comp_mode(Some(CompMode::Face));
        assert_eq!(app.s.comp_mode, Some(CompMode::Face));
        app.action("objectMode");
        assert_eq!(app.s.comp_mode, None);
        app.action("objectMode");
        assert_eq!(app.s.comp_mode, Some(CompMode::Face), "F8 must restore Face, not Vertex");
    }

    #[test]
    fn edit_pivot_ends_when_the_selection_is_cleared() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let id = app.s.create_mesh("polyCube", &inca_core::params::Params::new(), None);
        app.s.select(std::slice::from_ref(&id), inca_core::scene::SelMode::Replace);
        app.pivot_mode = true;
        app.end_pivot_mode_on_deselect();
        assert!(app.pivot_mode, "still on while the cube is selected");
        // a click on empty space / Deselect All clears the selection: Maya leaves Edit Pivot
        app.s.select(&[], inca_core::scene::SelMode::Replace);
        app.end_pivot_mode_on_deselect();
        assert!(!app.pivot_mode);
        // turning it on with nothing selected is not undone by this (no selection was lost)
        app.pivot_mode = true;
        app.end_pivot_mode_on_deselect();
        assert!(app.pivot_mode);
        // while D is held, clearing the selection ends the mode and the release keeps it off
        app.s.select(std::slice::from_ref(&id), inca_core::scene::SelMode::Replace);
        app.end_pivot_mode_on_deselect();
        app.pivot_hold = Some((egui::Key::D, true, 0.0, true));
        app.s.select(&[], inca_core::scene::SelMode::Replace);
        app.end_pivot_mode_on_deselect();
        assert!(!app.pivot_mode);
        assert_eq!(app.pivot_hold.map(|h| h.1), Some(false), "releasing D must not turn it back on");
    }

    #[test]
    fn edit_pivot_d_taps_toggle_and_holds_restore() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        let (id, press, release) = crate::hotkey_editor::hold_binding(&app, egui::Key::D, egui::Modifiers::default(), crate::hotkey_editor::Scope::Application)
            .expect("D is bound to Edit Pivot");
        assert_eq!(id, "pivotEdit");
        assert!(press && release, "D should run on both press and release");
        assert!(!app.pivot_mode);

        let run = |ctx: &egui::Context, app: &mut IncaApp, t: f64, pressed: bool| {
            let mut raw = raw_with(vec![key_event(egui::Key::D, false, pressed)]);
            raw.time = Some(t);
            let mut out = ctx.run_ui(raw, |ui| app.hotkeys(ui.ctx()));
            out.textures_delta.clear();
        };
        // A quick tap toggles pivot mode on, then another tap toggles it off.
        run(&ctx, &mut app, 1.0, true);
        assert!(app.pivot_mode);
        run(&ctx, &mut app, 1.1, false);
        assert!(app.pivot_mode, "quick tap should toggle pivot mode on");
        run(&ctx, &mut app, 2.0, true);
        run(&ctx, &mut app, 2.1, false);
        assert!(!app.pivot_mode, "second tap should toggle pivot mode off");
        // Holding the key enables pivot mode temporarily and restores it on release.
        run(&ctx, &mut app, 3.0, true);
        assert!(app.pivot_mode);
        run(&ctx, &mut app, 3.5, false);
        assert!(!app.pivot_mode, "held key should restore the previous pivot mode");
    }

    #[test]
    fn graph_copy_paste_hotkeys_work() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut curve = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        curve.set_key(1.0, 0.0, "linear");
        curve.set_key(10.0, 1.0, "linear");
        app.s.insert_anim_curve(ck.clone(), curve);
        app.windows_state.ge_keys = vec![(ck.clone(), 0)];
        app.win.graph_editor = true;
        app.windows_state.ge_window_rect = Some(egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(600.0, 400.0)));

        let mut out = ctx.run_ui(raw_with(vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0)), egui::Event::Copy]), |ui| app.hotkeys(ui.ctx()));
        out.textures_delta.clear();
        assert!(!app.windows_state.ge_clipboard.is_empty(), "Ctrl+C did not copy keys");

        app.s.time.current = 20.0;
        let mut out = ctx.run_ui(raw_with(vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0)), egui::Event::Paste("Inca keyframes".into())]), |ui| app.hotkeys(ui.ctx()));
        out.textures_delta.clear();
        let times: Vec<f64> = app.s.anim_curves()[&ck].keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![1.0, 10.0, 20.0], "Ctrl+V did not paste at the current time");
    }

    #[test]
    fn graph_copy_help_through_real_window() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        app.run_mel("test", "polyCube; setKeyframe -at translateX -t 1 -v 0; setKeyframe -at translateX -t 10 -v 1;").expect("scene setup");
        let ck = app.s.anim_curves().keys().find(|k| k.contains("translateX")).expect("curve").clone();
        app.windows_state.ge_keys = vec![(ck, 0)];
        app.win.graph_editor = true;
        let mut frame = eframe::Frame::_new_kittest();
        for _ in 0..4 {
            let mut out = ctx.run_ui(raw_with(vec![]), |ui| crate::windows::show(&mut app, ui.ctx(), &mut frame));
            out.textures_delta.clear();
        }
        let rect = app.windows_state.ge_window_rect.expect("Graph Editor window rect");
        let mut out = ctx.run_ui(raw_with(vec![egui::Event::PointerMoved(rect.center()), egui::Event::Copy]), |ui| app.hotkeys(ui.ctx()));
        out.textures_delta.clear();
        assert!(app.help.starts_with("Copied"), "help line was {:?}", app.help);
    }

    #[test]
    fn application_copy_paste_uses_graph_keys_when_selected() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut curve = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        curve.set_key(1.0, 0.0, "linear");
        curve.set_key(10.0, 1.0, "linear");
        app.s.insert_anim_curve(ck.clone(), curve);
        app.windows_state.ge_keys = vec![(ck.clone(), 0)];
        app.win.graph_editor = true;
        app.action("copy");
        assert!(!app.windows_state.ge_clipboard.is_empty(), "application copy did not defer to keys");
        app.s.time.current = 20.0;
        app.action("paste");
        let times: Vec<f64> = app.s.anim_curves()[&ck].keys.iter().map(|k| k.t).collect();
        assert_eq!(times, vec![1.0, 10.0, 20.0], "application paste did not defer to keys");
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn graph_copy_paste_click_flow() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        app.run_mel("test", "polyCube; setKeyframe -at translateX -t 1 -v 0; setKeyframe -at translateX -t 10 -v 1;").expect("scene setup");
        app.win.graph_editor = true;
        let run = |app: &mut IncaApp, frame: &mut eframe::Frame, events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(raw_with(events), |ui| eframe::App::ui(app, ui, frame));
            out.textures_delta.clear();
        };
        for _ in 0..5 {
            run(&mut app, &mut frame, vec![]);
        }
        let pos = app.windows_state.ge_first_key_screen.expect("first key screen position");
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(pos)]);
        run(&mut app, &mut frame, vec![pointer_button(pos, true)]);
        run(&mut app, &mut frame, vec![pointer_button(pos, false)]);
        assert!(!app.windows_state.ge_keys.is_empty(), "click did not select a key");
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(pos), egui::Event::Copy]);
        assert!(!app.windows_state.ge_clipboard.is_empty(), "Ctrl+C did not copy a key; help={:?}", app.help);
        app.s.time.current = 20.0;
        let bottom = egui::pos2(800.0, 950.0);
        run(&mut app, &mut frame, vec![egui::Event::PointerMoved(bottom), egui::Event::Paste("Inca keyframes".into())]);
        let ck = app.s.anim_curves().keys().find(|k| k.contains("translateX")).expect("curve").clone();
        let times: Vec<f64> = app.s.anim_curves()[&ck].keys.iter().map(|k| k.t).collect();
        assert!(times.contains(&20.0), "Ctrl+V did not paste at the current time: {times:?}");
    }

    #[test]
    fn timeline_copy_paste_actions_work() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        app.run_mel("test", "polyCube; setKeyframe -at translateX -t 10 -v 0; setKeyframe -at translateX -t 10 -v 1;").expect("scene setup");
        app.s.time.current = 10.0;
        app.action("timelineCopy");
        assert!(!app.key_clip.is_empty(), "timeline copy produced no clip");
        app.s.time.current = 20.0;
        app.action("timelinePaste");
        let ck = app.s.anim_curves().keys().find(|k| k.contains("translateX")).expect("curve").clone();
        let times: Vec<f64> = app.s.anim_curves()[&ck].keys.iter().map(|k| k.t).collect();
        assert!(times.contains(&20.0), "timeline paste did not create a key at 20: {times:?}");
    }

    #[test]
    fn graph_ctrl_c_works_with_text_focus() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = IncaApp::new(&cc);
        app.keymap.clear();
        let ck = inca_core::anim::Anim::key("pCube1", "translateX");
        let mut curve = inca_core::anim::AnimCurve::new("pCube1", "translateX");
        curve.set_key(1.0, 0.0, "linear");
        curve.set_key(10.0, 1.0, "linear");
        app.s.insert_anim_curve(ck.clone(), curve);
        app.windows_state.ge_keys = vec![(ck, 0)];
        app.win.graph_editor = true;
        app.windows_state.ge_window_rect = Some(egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(600.0, 400.0)));
        let mut text = String::new();
        let mut out = ctx.run_ui(raw_with(vec![]), |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
        });
        out.textures_delta.clear();
        assert!(ctx.egui_wants_keyboard_input(), "text field should have focus");
        app.hk.capturing = true;
        let mut out = ctx.run_ui(raw_with(vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0)), egui::Event::Copy]), |ui| app.hotkeys(ui.ctx()));
        out.textures_delta.clear();
        assert!(!app.windows_state.ge_clipboard.is_empty(), "Ctrl+C was swallowed by text focus");
        assert!(!app.hk.capturing, "editor copy should cancel hotkey capture");
    }
}