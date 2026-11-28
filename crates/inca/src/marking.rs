//! Marking menus: RMB object / component menus, Shift+RMB tool menus,
//! Ctrl+RMB selection conversion, W/E/R + LMB transform tool menus.

use crate::app::{IncaApp, Tool};
use crate::viewport::{self, CamView};
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Stroke};
use inca_core::scene::{CompMode, SelMode, Shape};

/// What picking an item does.
#[derive(Clone, PartialEq)]
pub enum Act {
    Action(String),
    Mode(Option<CompMode>),
    Select(String),
    AttrEditor(String),
    LookThrough(String),
    Frame(String),
    WorldSpace(bool),
    Discrete,
    Step(f64),
    Symmetry(usize),
    ToolSettings,
    ResetActive,
    ResetTool,
    AssignExisting(String),
    History(String, usize),
    SetTool(Tool),
    HypershadeAssign(String),
    HypershadeSelectUsers(String),
    HypershadeDuplicate(String),
    HypershadeRename(String),
    HypershadeGraphNetwork(String),
    HypershadeAddToGraph(String),
    HypershadeAttributeEditor(String),
    HypershadeShowNode(String),
    HypershadeLookThrough(String),
    HypershadeDelete(Vec<String>),
    HypershadeCreateMaterial(String),
    HypershadeCreateTexture(String),
    HypershadeCreatePlace,
    HypershadeFrameAll,
    HypershadeFrameSelected,
    HypershadeRearrange,
    HypershadeSelectObject(String),
    ConnectTexture(String, String, String, Pos2),
    BreakWire(crate::hypershade::ConnWire),
    IsolateWire(crate::hypershade::ConnWire),
    SelectWire(crate::hypershade::ConnWire),
}

#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub act: Option<Act>,
    pub sub: Vec<Item>,
    pub check: Option<bool>,
    pub enabled: bool,
}

fn it(label: &str, act: Act) -> Item {
    Item { label: label.into(), act: Some(act), sub: vec![], check: None, enabled: true }
}
fn cmd(label: &str, action: &str) -> Item {
    it(label, Act::Action(action.into()))
}
fn acmd(action: &str) -> Item {
    cmd(&crate::actions::label(action), action)
}
fn sub(label: &str, items: Vec<Item>) -> Item {
    Item { label: label.into(), act: None, sub: items, check: None, enabled: true }
}
fn checked(mut i: Item, on: bool) -> Item {
    i.check = Some(on);
    i
}
fn disabled(mut i: Item) -> Item {
    i.enabled = false;
    i
}

pub struct Menu {
    pub pos: Pos2,
    title: Option<String>,
    radial: Vec<(&'static str, Item)>,
    list: Vec<Option<Item>>,
    vp: usize,
    opened: f64,
    button: egui::PointerButton,
    sticky: bool,
    hot_radial: Option<usize>,
    /// Index of the open bottom-list submenu. It only opens from its parent row.
    list_sub: Option<usize>,
}

const DIRS: [(&str, f32); 8] = [("N", -90.0), ("NE", -45.0), ("E", 0.0), ("SE", 45.0), ("S", 90.0), ("SW", 135.0), ("W", 180.0), ("NW", -135.0)];

fn menu(pos: Pos2, vp: usize, button: egui::PointerButton, title: Option<String>, radial: Vec<(&'static str, Item)>, list: Vec<Option<Item>>, now: f64) -> Menu {
    Menu { pos, title, radial, list, vp, opened: now, button, sticky: false, hot_radial: None, list_sub: None }
}

/// RMB in a viewport: build the menu for the modifiers and what's under the cursor.
pub fn open_rmb(app: &mut IncaApp, idx: usize, cv: &CamView, pos: Pos2, mods: egui::Modifiers, now: f64) {
    let r = egui::PointerButton::Secondary;
    let vp = app.vps[idx].clone();
    // Shift+RMB: tool menus per component mode
    if mods.shift && !mods.ctrl {
        let (radial, list): (Vec<(&'static str, Item)>, Vec<Option<Item>>) = match app.s.comp_mode {
            Some(CompMode::Face) => (
                vec![("N", cmd("Extrude Face", "extrude")), ("NE", cmd("Duplicate Face", "duplicateFaces")), ("E", cmd("Extract Face", "extract")), ("S", cmd("Poke Face", "poke")), ("W", cmd("Bridge", "bridge")), ("NW", cmd("Bevel Face", "bevel")), ("SW", cmd("Add Divisions", "addDivisions")), ("SE", cmd("Triangulate", "triangulate"))],
                vec![Some(acmd("multiCutTool")), Some(acmd("insertEdgeLoopTool")), None, Some(acmd("toVertices")), Some(acmd("toEdges")), None, Some(sub("Assign New Material", new_mats()))],
            ),
            Some(CompMode::Edge) => (
                vec![("N", cmd("Extrude Edge", "extrude")), ("NE", cmd("Collapse Edge", "collapse")), ("E", cmd("Bridge Edge", "bridge")), ("S", cmd("Delete Edge", "deleteEdgeVertex")), ("W", cmd("Bevel Edge", "bevel")), ("NW", cmd("Merge Edges", "mergeToCenter")), ("SW", sub("Soften/Harden", vec![acmd("softenEdge"), acmd("hardenEdge")])), ("SE", cmd("Fill Hole", "fillHole"))],
                vec![Some(acmd("insertEdgeLoopTool")), Some(acmd("multiCutTool")), Some(acmd("connect")), None, Some(acmd("selectEdgeLoop")), Some(acmd("selectEdgeRing")), Some(acmd("selectBorder"))],
            ),
            Some(CompMode::Vertex) => (
                vec![("N", cmd("Extrude Vertex", "chamferVertex")), ("NE", cmd("Merge Vertices", "merge")), ("E", cmd("Connect", "connect")), ("S", cmd("Delete Vertex", "deleteEdgeVertex")), ("W", cmd("Chamfer Vertex", "chamferVertex")), ("NW", cmd("Merge to Center", "mergeToCenter")), ("SW", cmd("Target Weld Tool", "targetWeldTool"))],
                vec![Some(acmd("multiCutTool")), Some(acmd("insertEdgeLoopTool")), None, Some(acmd("toEdges")), Some(acmd("toFaces"))],
            ),
            None => (
                vec![("N", cmd("Extrude", "extrude")), ("NE", cmd("Combine", "combine")), ("E", cmd("Separate", "separate")), ("S", cmd("Smooth", "smooth")), ("W", cmd("Mirror", "mirror")), ("NW", cmd("Bevel", "bevel")), ("SE", cmd("Delete History", "deleteHistory")), ("SW", cmd("Bridge", "bridge"))],
                vec![Some(acmd("multiCutTool")), Some(acmd("insertEdgeLoopTool")), Some(acmd("targetWeldTool")), None, Some(acmd("centerPivot")), Some(acmd("freezeTransformations")), Some(acmd("resetTransformations"))],
            ),
        };
        app.marking = Some(menu(pos, idx, r, None, radial, list, now));
        return;
    }
    // Ctrl+RMB: selection conversion
    if mods.ctrl || mods.command {
        let radial = vec![("N", cmd("To Edges", "toEdges")), ("W", cmd("To Vertices", "toVertices")), ("E", cmd("To Faces", "toFaces")), ("S", cmd("Edge Loop", "selectEdgeLoop")), ("SE", cmd("Edge Ring", "selectEdgeRing")), ("SW", cmd("Border", "selectBorder")), ("NE", cmd("Grow", "growSelection")), ("NW", cmd("Shrink", "shrinkSelection"))];
        app.marking = Some(menu(pos, idx, r, None, radial, vec![], now));
        return;
    }
    let s = &app.s;
    let off = &app.sel_mask_off;
    // Templates never get the object menu, even when the Template selection mask is on.
    let under = viewport::pick_object_if(s, &vp, cv, pos, &mut app.mesh_cache, &|id| !s.is_template(id) && crate::toolbar::mask_allows_scene(s, off, id));
    // Templates get no component modes, but keep the object list menu.
    let template = if under.is_none() {
        viewport::pick_object_if(s, &vp, cv, pos, &mut app.mesh_cache, &|id| s.is_template(id) && crate::toolbar::mask_allows_scene(s, off, id))
    } else {
        None
    };
    let target = under.clone().filter(|o| app.s.node(o).is_some_and(|n| matches!(n.shape, Shape::Mesh(_) | Shape::Curve(_)))).or_else(|| app.s.comp_mode.and_then(|_| app.s.hilite().first().cloned()));
    if let Some(t) = target {
        let is_curve = matches!(app.s.node(&t).map(|n| &n.shape), Some(Shape::Curve(_)));
        let name = app.s.name_of(&t);
        let radial = if is_curve {
            vec![("N", it("Control Vertex", Act::Mode(Some(CompMode::Vertex)))), ("E", it("Object Mode", Act::Mode(None))), ("W", it("Edit Point", Act::Mode(Some(CompMode::Vertex)))), ("S", disabled(it("Curve Point", Act::Mode(Some(CompMode::Vertex))))), ("NE", it("Select", Act::Select(t.clone())))]
        } else {
            vec![("N", it("Edge", Act::Mode(Some(CompMode::Edge)))), ("W", it("Vertex", Act::Mode(Some(CompMode::Vertex)))), ("S", it("Face", Act::Mode(Some(CompMode::Face)))), ("E", it("Object Mode", Act::Mode(None))), ("SE", disabled(it("UV", Act::Mode(Some(CompMode::Vertex))))), ("NE", it("Select", Act::Select(t.clone()))), ("NW", it("Multi", Act::Mode(Some(CompMode::Vertex)))), ("SW", it("Vertex Face", Act::Mode(Some(CompMode::Vertex))))]
        };
        let list = object_menu_list(app, &t);
        // picking from this menu works on the object under the cursor
        app.marking_target = Some(t);
        app.marking = Some(menu(pos, idx, r, Some(format!("{name}...")), radial, list, now));
        return;
    }
    if let Some(o) = under {
        let name = app.s.name_of(&o);
        let cam = matches!(app.s.node(&o).map(|n| &n.shape), Some(Shape::Camera(_)));
        let radial = vec![("N", it("Select", Act::Select(o.clone()))), ("E", it("Attribute Editor", Act::AttrEditor(o.clone()))), ("W", if cam { it("Look Through", Act::LookThrough(name.clone())) } else { it("Frame", Act::Frame(o.clone())) })];
        let list = vec![Some(cmd("Select All", "selectAll")), Some(cmd("Select Hierarchy", "selectHierarchy")), None, Some(acmd("hideSelection")), Some(cmd("Delete", "deleteSel"))];
        app.marking = Some(menu(pos, idx, r, Some(format!("{name}...")), radial, list, now));
        return;
    }
    if let Some(t) = template {
        let name = app.s.name_of(&t);
        let radial = vec![("N", cmd("Complete Tool", "finishTool")), ("S", cmd("Select All", "selectAll"))];
        let list = object_menu_list(app, &t);
        app.marking_target = Some(t);
        app.marking = Some(menu(pos, idx, r, Some(format!("{name}...")), radial, list, now));
        return;
    }
    // Maya's no-object marking menu: Complete Tool at N, Select All at S, nothing else.
    let radial = vec![("N", cmd("Complete Tool", "finishTool")), ("S", cmd("Select All", "selectAll"))];
    app.marking = Some(menu(pos, idx, r, None, radial, vec![], now));
}

/// The list half of the object marking menu (Select All, materials, inputs...).
fn object_menu_list(app: &IncaApp, t: &str) -> Vec<Option<Item>> {
    let inputs: Vec<Item> = app.s.history_metas(t).iter().enumerate().rev().filter(|(_, h)| !h.name.contains('#') && inca_core::ops::info(&h.ty).is_some_and(|i| !i.1)).map(|(i, h)| it(&h.name, Act::History(t.to_string(), i))).collect();
    let shape = app.s.node(t).and_then(|n| n.shape_name.clone()).unwrap_or("(none)".into());
    vec![
        Some(it("Select", Act::Select(t.to_string()))),
        Some(cmd("Select All", "selectAll")),
        Some(cmd("Deselect All", "deselectAll")),
        Some(cmd("Select Hierarchy", "selectHierarchy")),
        Some(cmd("Invert Selection", "invertSelection")),
        None,
        Some(sub("Assign New Material...", new_mats())),
        Some(sub("Assign Existing Material", app.s.materials().into_iter().map(|m| it(&m.name, Act::AssignExisting(m.name.clone()))).collect())),
        Some(it("Material Attributes...", Act::Action("hypershade".into()))),
        None,
        Some(sub("Actions", ["hideSelection", "showLastHidden", "templateSel", "centerPivot", "freezeTransformations", "deleteHistory"].iter().map(|a| acmd(a)).collect())),
        Some(sub("Inputs", inputs)),
        Some(sub("Outputs", vec![disabled(cmd(&shape, ""))])),
        None,
        Some(sub("Paint", vec![it("Paint Select Tool", Act::SetTool(Tool::Paint)), cmd("Sculpt Tool", "sculptTool")])),
        Some(disabled(cmd("Metadata", ""))),
    ]
}

fn new_mats() -> Vec<Item> {
    [("Lambert", "assignNew_lambert"), ("Blinn", "assignNew_blinn"), ("Phong", "assignNew_phong"), ("Standard Surface", "assignNew_standardSurface"), ("Surface Shader", "assignNew_surfaceShader")].iter().map(|(l, a)| cmd(l, a)).collect()
}

/// W / E / R + LMB: transform tool marking menu.
pub fn open_tool(app: &mut IncaApp, idx: usize, tool: Tool, pos: Pos2, now: f64) {
    app.set_tool(tool);
    let (discrete, steps, unit): (&str, &[f64], &str) = match tool {
        Tool::Rotate => ("Discrete Rotate", &[1.0, 5.0, 10.0, 15.0, 30.0, 45.0, 90.0], "°"),
        Tool::Scale => ("Discrete Scale", &[0.1, 0.25, 0.5, 1.0], ""),
        _ => ("Discrete Move", &[0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0], ""),
    };
    let step = match tool {
        Tool::Rotate => app.vp_state.snap_angle,
        Tool::Scale => app.vp_state.snap_scale,
        _ => app.vp_state.snap_step,
    };
    let mi = crate::vp_ui::mode_index(tool);
    let sym = ["Off", "Object X", "World X", "World Y", "World Z"];
    let radial = vec![
        ("N", checked(it("World", Act::WorldSpace(true)), app.space_world(tool))),
        ("NE", checked(it("Object", Act::WorldSpace(false)), !app.space_world(tool))),
        ("E", checked(cmd("Edit Pivot", "pivotEdit"), app.pivot_mode)),
        ("SE", checked(cmd("Soft Select", "softSelect"), app.soft.on)),
        ("S", it("Tool Settings...", Act::ToolSettings)),
        ("SW", checked(it(discrete, Act::Discrete), app.vp_state.discrete[mi])),
        ("W", sub("Step Size", steps.iter().map(|&v| checked(it(&format!("{}{unit}", inca_core::math::fmt_num(v)), Act::Step(v)), (step - v).abs() < 1e-9)).collect())),
        ("NW", sub("Symmetry", sym.iter().enumerate().map(|(i, l)| checked(it(l, Act::Symmetry(i)), app.symmetry == i)).collect())),
    ];
    let list = vec![
        Some(if app.vp_state.active_handle(app.tool).is_some() { it("Reset Active Handle", Act::ResetActive) } else { disabled(it("Reset Active Handle", Act::ResetActive)) }),
        Some(it("Reset Tool", Act::ResetTool)),
    ];
    let title = crate::actions::label(match tool {
        Tool::Rotate => "rotateTool",
        Tool::Scale => "scaleTool",
        _ => "moveTool",
    });
    app.marking = Some(menu(pos, idx, egui::PointerButton::Primary, Some(title), radial, list, now));
}

/// Block clicks from reaching the UI under a painter-drawn marking menu.
pub fn block_pointer(app: &IncaApp, ctx: &egui::Context) {
    if app.marking.is_none() {
        return;
    }
    let rect = ctx.content_rect();
    egui::Area::new(egui::Id::new("marking-block"))
        .order(egui::Order::Tooltip)
        .fixed_pos(rect.min)
        .interactable(true)
        .show(ctx, |ui| {
            ui.allocate_exact_size(rect.size(), egui::Sense::click());
        });
}

/// Open a marking menu outside a viewport (used by the Hypershade).
pub fn open_custom(app: &mut IncaApp, pos: Pos2, title: Option<String>, radial: Vec<(&'static str, Item)>, list: Vec<Option<Item>>, now: f64, sticky: bool) {
    let mut m = menu(pos, app.active_vp, egui::PointerButton::Secondary, title, radial, list, now);
    m.sticky = sticky;
    app.marking = Some(m);
    app.marking_target = None;
}

/// A marking submenu floats over the marking menu like the toolbar/menu-bar popups.
fn paint_popup_frame(p: &egui::Painter, rect: Rect, visuals: &egui::Visuals) {
    p.add(visuals.popup_shadow.as_shape(rect, visuals.menu_corner_radius));
    p.rect_filled(rect, visuals.menu_corner_radius, visuals.window_fill);
    p.rect_stroke(rect, visuals.menu_corner_radius, visuals.window_stroke, egui::StrokeKind::Inside);
}

fn run(app: &mut IncaApp, act: Act) {
    let vp = app.marking.as_ref().map(|m| m.vp).unwrap_or(app.active_vp);
    // menus opened over an object act on it
    if let Some(t) = app.marking_target.take() {
        if matches!(act, Act::Mode(_) | Act::Action(_) | Act::AssignExisting(_)) && !app.s.sel.contains(&t) && !app.s.hilite().contains(&t) {
            if app.s.comp_mode.is_some() {
                app.set_comp_mode(None);
            }
            app.select_names(std::slice::from_ref(&t), SelMode::Replace);
        }
    }
    match act {
        Act::Action(a) if !a.is_empty() => {
            app.active_vp = vp;
            app.action(&a);
        }
        Act::Action(_) => {}
        Act::Mode(m) => app.set_comp_mode(m),
        Act::Select(o) => {
            if app.s.comp_mode.is_some() {
                app.set_comp_mode(None);
            } else {
                app.s.checkpoint_selection("select");
            }
            app.select_names(&[o], SelMode::Replace);
        }
        Act::AttrEditor(o) => {
            app.select_names(&[o], SelMode::Replace);
            app.action("attributeEditor");
        }
        Act::LookThrough(c) => {
            app.push_view_change(vp);
            app.vps[vp].camera = c;
        }
        Act::Frame(o) => {
            app.select_names(&[o], SelMode::Replace);
            app.active_vp = vp;
            app.action("frameSelected");
        }
        Act::WorldSpace(w) => app.set_space_world(app.tool, w),
        Act::Discrete => {
            let i = crate::vp_ui::mode_index(app.tool);
            app.vp_state.discrete[i] = !app.vp_state.discrete[i];
        }
        Act::Step(v) => {
            let i = match app.tool {
                Tool::Rotate => {
                    app.vp_state.snap_angle = v;
                    1
                }
                Tool::Scale => {
                    app.vp_state.snap_scale = v;
                    2
                }
                _ => {
                    app.vp_state.snap_step = v;
                    0
                }
            };
            app.vp_state.discrete[i] = true;
        }
        Act::Symmetry(i) => app.symmetry = i,
        Act::ToolSettings => {
            app.right_tab = crate::app::RightTab::ToolSettings;
            app.show_right = true;
        }
        Act::ResetActive => {
            app.vp_state.reset_active_handle(app.tool);
        }
        Act::ResetTool => {
            // JS Reset Tool: move goes back to world, rotate and scale to object space; this
            // tool's discrete flag and step reset (the grid toggle and other tools' steps stay)
            app.set_space_world(app.tool, app.tool == Tool::Move);
            let i = crate::vp_ui::mode_index(app.tool);
            app.vp_state.discrete[i] = false;
            match app.tool {
                Tool::Rotate => app.vp_state.snap_angle = 15.0,
                Tool::Scale => app.vp_state.snap_scale = 1.0,
                _ => app.vp_state.snap_step = 1.0,
            }
            app.pivot_mode = false;
            app.vp_state.reset_active_handle(app.tool);
        }
        Act::AssignExisting(m) => {
            let _ = app.run_mel("assign material", &format!("hyperShade -assign {m};"));
        }
        Act::History(id, i) => {
            app.select_names(std::slice::from_ref(&id), SelMode::Replace);
            app.inview = Some((id.clone(), i));
            app.manip_node = Some((id.clone(), i));
            app.show_manips_tool = true;
            if app.s.history_meta(&id, i).is_some_and(|h| h.ty == "polyExtrudeFace") {
                app.extrude_manip = Some((id, i));
                app.extrude_forced = true;
            } else {
                app.extrude_manip = None;
                app.extrude_forced = false;
            }
        }
        Act::SetTool(t) => app.set_tool(t),
        Act::HypershadeAssign(id) => {
            if let Some(m) = app.s.material(&id) {
                let _ = app.run_mel("assign material", &format!("hyperShade -assign {};", m.name));
            }
        }
        Act::HypershadeSelectUsers(id) => {
            let ids = crate::hypershade::material_users(app, &id);
            app.select_names(&ids, SelMode::Replace);
        }
        Act::HypershadeDuplicate(id) => crate::hypershade::duplicate_shading_network(app, &id),
        Act::HypershadeRename(id) => {
            if let Some(name) = app.s.material(&id).map(|m| m.name).or_else(|| app.s.texture(&id).map(|t| t.name)).or_else(|| app.s.node(&id).map(|n| n.name.clone())) {
                app.windows_state.hs.rename = Some((id, name));
            }
        }
        Act::HypershadeGraphNetwork(id) => crate::hypershade::graph_node_network(app, &id),
        Act::HypershadeAddToGraph(id) => {
            if let Some(graph) = &mut app.windows_state.hs.graph_ids {
                graph.insert(id);
            }
        }
        Act::HypershadeAttributeEditor(id) => {
            if app.s.is_material(&id) || app.s.is_texture(&id) {
                crate::hypershade::select_shading_node(app, &id);
            } else {
                app.select_names(std::slice::from_ref(&id), SelMode::Replace);
                app.windows_state.hs_selected = Some(id);
                app.windows_state.hs.graph_sel.clear();
            }
            app.show_right = true;
            app.right_tab = crate::app::RightTab::AttributeEditor;
        }
        Act::HypershadeShowNode(id) => {
            if app.s.is_material(&id) || app.s.is_texture(&id) {
                crate::hypershade::select_shading_node(app, &id);
            } else {
                app.select_names(std::slice::from_ref(&id), SelMode::Replace);
                app.windows_state.hs_selected = Some(id);
                app.windows_state.hs.graph_sel.clear();
            }
        }
        Act::HypershadeLookThrough(id) => {
            let name = app.s.name_of(&id);
            if !name.is_empty() {
                app.push_view_change(vp);
                app.vps[vp].camera = name;
            }
        }
        Act::HypershadeDelete(ids) => {
            let ids: std::collections::HashSet<String> = ids.into_iter().collect();
            crate::hypershade::delete_graph_nodes(app, &ids);
        }
        Act::HypershadeCreateMaterial(ty) => {
            let _ = app.run_mel("create material", &format!("shadingNode -asShader {ty};"));
            if let Some(id) = app.s.material_ids().last().cloned() {
                let at = app.windows_state.hs.create_pos.take();
                crate::hypershade::place_new_node(app, &id, at);
                app.windows_state.hs_selected = Some(id);
            }
        }
        Act::HypershadeCreateTexture(ty) => {
            let _ = app.run_mel("create texture", &format!("shadingNode -asTexture {ty};"));
            if let Some(id) = app.s.texture_ids().last().cloned() {
                let at = app.windows_state.hs.create_pos.take();
                crate::hypershade::place_new_node(app, &id, at);
                app.windows_state.hs_selected = Some(id);
            }
        }
        Act::HypershadeCreatePlace => {
            let at = app.windows_state.hs.create_pos.take();
            crate::hypershade::create_place_node(app, at);
        }
        Act::HypershadeFrameAll => app.windows_state.hs.frame = Some(false),
        Act::HypershadeFrameSelected => app.windows_state.hs.frame = Some(true),
        Act::HypershadeRearrange => {
            app.windows_state.hs.pos.clear();
            app.windows_state.hs.frame = Some(false);
        }
        Act::HypershadeSelectObject(id) => {
            app.s.checkpoint_selection("select");
            app.select_names(&[id], SelMode::Replace);
        }
        Act::ConnectTexture(tex, mid, attr, pos) => {
            crate::hypershade::ensure_graph_node(app, &tex, pos);
            crate::hypershade::connect_texture(app, &tex, &mid, &attr);
        }
        Act::BreakWire(w) => crate::hypershade::break_connection(app, &w),
        Act::IsolateWire(w) => crate::hypershade::isolate_connection(app, &w),
        Act::SelectWire(w) => crate::hypershade::select_connection(app, &w),
    }
}

const ROW: f32 = 31.0;

fn radial_rect(painter_fonts: &egui::Context, m: &Menu, dir: &str, label: &str) -> Rect {
    let row = match dir {
        "N" => -2.0,
        "S" => 2.0,
        d if d.starts_with('N') => -1.0,
        d if d.starts_with('S') => 1.0,
        _ => 0.0,
    };
    let side = if dir == "N" || dir == "S" { 0.0 } else if dir.ends_with('W') { -1.0 } else { 1.0 };
    let w = painter_fonts.fonts_mut(|f| f.layout_no_wrap(label.to_string(), FontId::proportional(12.0), Color32::WHITE).size().x) + 22.0;
    let gap = if row != 0.0 { 30.0 } else { 46.0 };
    let cx = m.pos.x + side * gap;
    let cy = m.pos.y + row * ROW;
    let x0 = if side < 0.0 { cx - w } else if side > 0.0 { cx } else { cx - w / 2.0 };
    Rect::from_min_size(pos2(x0, cy - 11.0), vec2(w, 22.0))
}

/// Draw and drive the open marking menu.
pub fn show(app: &mut IncaApp, ctx: &egui::Context) {
    let Some(m) = app.marking.as_ref() else { return };
    // Losing focus (Alt-Tab, Windows key) can swallow the matching release/click, which would
    // leave a sticky marking menu locked open until Esc.
    if !ctx.input(|i| i.focused) {
        app.marking = None;
        app.marking_target = None;
        return;
    }
    let (ptr, now, esc) = ctx.input(|i| (i.pointer.clone(), i.time, i.key_pressed(egui::Key::Escape)));
    if esc {
        app.marking = None;
        app.marking_target = None;
        return;
    }
    let cur = ptr.hover_pos().unwrap_or(m.pos);
    let d = cur - m.pos;
    let rects: Vec<Rect> = m.radial.iter().map(|(d, i)| radial_rect(ctx, m, d, &i.label)).collect();
    let list_top = if m.radial.is_empty() { m.pos.y } else { m.pos.y + 2.0 * ROW + 22.0 };
    let has_list = !m.list.is_empty() || m.title.is_some();
    let list_h = 8.0 + m.title.as_ref().map_or(0.0, |_| 18.0) + m.list.iter().map(|i| if i.is_some() { 20.0 } else { 7.0 }).sum::<f32>();
    let list_rect = if has_list {
        Rect::from_min_size(pos2(m.pos.x - 85.0, list_top), vec2(170.0, list_h))
    } else {
        Rect::NOTHING
    };
    // Once the pointer is in the bottom menu (or one of its submenus), the radial
    // gesture is over: no hot item and no gesture line, just a normal menu.
    let in_list = has_list && (list_rect.contains(cur) || cur.y > list_top - 6.0);
    // hot radial item: under the cursor, or by direction once past the dead zone
    let mut hot = if in_list { None } else { rects.iter().position(|r| r.contains(cur)) };
    if hot.is_none() && d.length() > 28.0 && !in_list {
        let ang = d.y.atan2(d.x).to_degrees();
        let best = m.radial.iter().enumerate().map(|(i, (dir, _))| {
            let a = DIRS.iter().find(|x| x.0 == *dir).unwrap().1;
            (i, ((ang - a + 540.0) % 360.0 - 180.0).abs())
        }).min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, diff)) = best {
            if diff < 28.0 {
                hot = Some(i);
            }
        }
    }
    // A radial submenu is a normal menu popup: it overlaps the item slightly and casts the
    // same drop shadow as the toolbar/menu-bar popups.
    let sub_rect = |i: usize| -> Rect {
        let r = rects[i];
        let n = m.radial[i].1.sub.len() as f32;
        let left = m.radial[i].0.ends_with('W');
        let w = 130.0;
        let x0 = if left { r.min.x - w + 3.0 } else { r.max.x - 3.0 };
        Rect::from_min_size(pos2(x0, r.min.y), vec2(w, n * 20.0 + 6.0))
    };
    // A radial submenu stays hot while the pointer is in it. The gesture line ends here too:
    // once the pointer is in a submenu the radial gesture is over, even if the submenu hangs
    // below the start of the bottom list.
    let mut in_radial_sub = false;
    if let Some(h) = [hot, m.hot_radial]
        .into_iter()
        .flatten()
        .find(|h| !m.radial[*h].1.sub.is_empty() && sub_rect(*h).contains(cur))
    {
        hot = Some(h);
        in_radial_sub = true;
    }
    let mut picked: Option<Act> = None;
    let mut close = false;
    let primary_click = ptr.button_clicked(egui::PointerButton::Primary);
    let list = m.list.clone();
    let title = m.title.clone();
    let radial = m.radial.clone();
    let pos = m.pos;
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("marking"));
    let p = ctx.layer_painter(layer);
    let visuals = ctx.global_style().visuals.clone();
    let mut sub_hit: Option<Act> = None;
    for (i, ((_, item), r)) in radial.iter().zip(&rects).enumerate() {
        let is_hot = hot == Some(i);
        let bg = if !item.enabled { Color32::from_gray(0x45) } else if is_hot { Color32::from_rgb(0x52, 0x85, 0xa6) } else { Color32::from_gray(0x50) };
        p.rect_filled(*r, 3.0, bg);
        p.rect_stroke(*r, 3.0, Stroke::new(1.0, Color32::from_gray(0x2a)), egui::StrokeKind::Inside);
        let mut label = item.label.clone();
        if let Some(c) = item.check {
            label = format!("{} {label}", if c { "✔" } else { " " });
        }
        if !item.sub.is_empty() {
            // egui's bundled fonts support these triangles; unfilled ones like "▸" render as a square.
            label = if radial[i].0.ends_with('W') { format!("⏴ {label}") } else { format!("{label} {}", egui::menu::SubMenuButton::RIGHT_ARROW) };
        }
        p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(12.0), if item.enabled { Color32::WHITE } else { Color32::from_gray(0x80) });
        if is_hot && item.enabled && item.sub.is_empty() && primary_click {
            picked = item.act.clone();
        }
    }
    // the list below the radial items
    let button = m.button;
    let released = ptr.button_released(button);
    let mut open_list_sub: Option<usize> = None;
    if has_list {
        let r = list_rect;
        p.rect_filled(r, 3.0, Color32::from_gray(0x44));
        p.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_gray(0x2a)), egui::StrokeKind::Inside);
        let mut y = r.min.y + 4.0;
        if let Some(t) = &title {
            p.text(pos2(r.min.x + 6.0, y + 9.0), Align2::LEFT_CENTER, t, FontId::proportional(12.0), Color32::from_gray(0xaa));
            y += 18.0;
        }
        let mut rows: Vec<Option<Rect>> = Vec::with_capacity(list.len());
        for i in &list {
            match i {
                None => {
                    rows.push(None);
                    y += 7.0;
                }
                Some(_) => {
                    rows.push(Some(Rect::from_min_size(pos2(r.min.x + 3.0, y), vec2(r.width() - 6.0, 20.0))));
                    y += 20.0;
                }
            }
        }
        let screen = ctx.content_rect();
        let mut sub_rects: Vec<Option<Rect>> = vec![None; list.len()];
        for (idx, item) in list.iter().enumerate() {
            let Some(item) = item else { continue };
            if item.sub.is_empty() {
                continue;
            }
            let Some(rr) = rows[idx] else { continue };
            let sw = 150.0;
            let sh = 6.0 + item.sub.len() as f32 * 20.0;
            let left = rr.max.x + sw + 4.0 > screen.max.x;
            let sx = if left { rr.min.x - sw + 3.0 } else { rr.max.x - 3.0 };
            sub_rects[idx] = Some(Rect::from_min_size(pos2(sx, rr.min.y), vec2(sw, sh)));
        }
        // Only the parent row opens a list submenu. Once open, moving sideways into it keeps
        // it open; approaching another submenu from the side does not open it.
        let mut open_sub: Option<(usize, Rect)> = None;
        if let Some(prev) = m.list_sub {
            if let Some(sr) = sub_rects.get(prev).copied().flatten() {
                if sr.contains(cur) {
                    open_sub = Some((prev, sr));
                }
            }
        }
        for (idx, item) in list.iter().enumerate() {
            let Some(item) = item else { continue };
            if item.sub.is_empty() || !rows[idx].is_some_and(|rr| rr.contains(cur)) {
                continue;
            }
            if let Some(sr) = sub_rects[idx] {
                open_sub = Some((idx, sr));
                break;
            }
        }
        open_list_sub = open_sub.map(|(i, _)| i);
        for (idx, item) in list.iter().enumerate() {
            let Some(item) = item else { continue };
            let Some(rr) = rows[idx] else { continue };
            let open_parent = open_sub.is_some_and(|(oi, _)| oi == idx);
            if rr.contains(cur) || open_parent {
                p.rect_filled(rr, 2.0, Color32::from_rgb(0x52, 0x85, 0xa6));
            }
            let l = match item.check {
                Some(true) => format!("✔ {}", item.label),
                Some(false) => format!("   {}", item.label),
                None => item.label.clone(),
            };
            let col = if item.enabled { Color32::WHITE } else { Color32::from_gray(0x80) };
            p.text(pos2(rr.min.x + 6.0, rr.center().y), Align2::LEFT_CENTER, l, FontId::proportional(12.0), col);
            if !item.sub.is_empty() {
                let c = pos2(rr.max.x - 9.0, rr.center().y);
                p.add(egui::Shape::convex_polygon(
                    vec![pos2(c.x - 2.5, c.y - 4.0), pos2(c.x + 3.5, c.y), pos2(c.x - 2.5, c.y + 4.0)],
                    col,
                    Stroke::NONE,
                ));
            }
            if item.sub.is_empty() && rr.contains(cur) && item.enabled && (primary_click || released) {
                picked = item.act.clone();
            }
        }
        if let Some((idx, sr)) = open_sub {
            let Some(Some(item)) = list.get(idx) else { return };
            paint_popup_frame(&p, sr, &visuals);
            for (k, s) in item.sub.iter().enumerate() {
                let rr = Rect::from_min_size(pos2(sr.min.x + 3.0, sr.min.y + 3.0 + k as f32 * 20.0), vec2(sr.width() - 6.0, 20.0));
                if rr.contains(cur) {
                    p.rect_filled(rr, 2.0, Color32::from_rgb(0x52, 0x85, 0xa6));
                    if s.enabled && (primary_click || released) {
                        picked = s.act.clone();
                    }
                }
                let l = match s.check {
                    Some(true) => format!("✔ {}", s.label),
                    Some(false) => format!("   {}", s.label),
                    None => s.label.clone(),
                };
                p.text(pos2(rr.min.x + 6.0, rr.center().y), Align2::LEFT_CENTER, l, FontId::proportional(12.0), Color32::WHITE);
            }
        }
    }
    // Draw the open radial submenu last: it is a floating popup and must cover the bottom list
    // too, not just the radial items.
    if let Some(h) = hot {
        let item = &radial[h].1;
        if !item.sub.is_empty() {
            let sr = sub_rect(h);
            paint_popup_frame(&p, sr, &visuals);
            for (k, s) in item.sub.iter().enumerate() {
                let rr = Rect::from_min_size(pos2(sr.min.x + 3.0, sr.min.y + 3.0 + k as f32 * 20.0), vec2(sr.width() - 6.0, 20.0));
                if rr.contains(cur) {
                    p.rect_filled(rr, 2.0, Color32::from_rgb(0x52, 0x85, 0xa6));
                    sub_hit = s.act.clone();
                    if s.enabled && primary_click {
                        picked = s.act.clone();
                    }
                }
                let l = match s.check {
                    Some(true) => format!("✔ {}", s.label),
                    Some(false) => format!("   {}", s.label),
                    None => s.label.clone(),
                };
                p.text(pos2(rr.min.x + 6.0, rr.center().y), Align2::LEFT_CENTER, l, FontId::proportional(12.0), Color32::WHITE);
            }
        }
    }
    // The drag line is drawn last so it stays visible over every option, except when the
    // pointer is in the bottom menu or a radial submenu (there it behaves like a normal menu).
    if !m.radial.is_empty() && !in_list && !in_radial_sub {
        p.line_segment([pos, cur], Stroke::new(1.5, Color32::from_gray(0xdd)));
        p.circle_filled(pos, 4.0, Color32::from_gray(0xdd));
    }
    let menu_open = ctx.any_popup_open();
    let quick = now - app.marking.as_ref().unwrap().opened < 0.26 && d.length() < 6.0;
    let sticky = app.marking.as_ref().unwrap().sticky;
    if picked.is_none() {
        if released && !sticky {
            if quick {
                if button == egui::PointerButton::Primary {
                    app.marking.as_mut().unwrap().sticky = true;
                } else {
                    close = true;
                }
            } else if let Some(a) = sub_hit.clone() {
                picked = Some(a);
            } else if let Some(h) = hot {
                if radial[h].1.enabled && radial[h].1.sub.is_empty() {
                    picked = radial[h].1.act.clone();
                } else if !radial[h].1.sub.is_empty() {
                    if button == egui::PointerButton::Primary {
                        app.marking.as_mut().unwrap().sticky = true;
                    } else {
                        close = true;
                    }
                } else {
                    close = true;
                }
            } else {
                close = true;
            }
        } else if sticky && ptr.any_click() {
            if let Some(a) = sub_hit {
                picked = Some(a);
            } else if let Some(h) = hot.filter(|h| rects[*h].contains(cur)) {
                if radial[h].1.enabled && radial[h].1.sub.is_empty() {
                    picked = radial[h].1.act.clone();
                }
            } else if !list_rect.contains(cur) && !menu_open {
                close = true;
            }
        }
    }
    if let Some(m) = app.marking.as_mut() {
        m.hot_radial = hot;
        m.list_sub = open_list_sub;
    }
    if let Some(a) = picked {
        run(app, a);
        close = true;
    }
    if close {
        app.marking = None;
        app.marking_target = None;
    }
    ctx.request_repaint();
}


