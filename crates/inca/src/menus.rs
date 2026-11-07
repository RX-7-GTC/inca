//! Main menu bar with Maya menu sets (Modeling, Rigging, Animation, FX, Rendering).

use crate::actions::label;
use crate::app::{IncaApp, MenuSet};
use inca_core::primitives::GEN_INFO;
use inca_core::scene::LIGHT_TYPES;

/// Which menu bar an open menu belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuSource {
    Main,
    Hotbox,
    Outliner,
    AttrEditor,
    RenderView,
    UvEditor,
    Hypershade,
    Panel(usize),
}

impl MenuSource {
    fn with(self, title: &'static str) -> OpenMenu {
        OpenMenu { source: self, title }
    }
}

/// The top-level menu that is currently open (Maya keeps exactly one open at a time).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenMenu {
    pub source: MenuSource,
    pub title: &'static str,
}

pub enum M {
    A(&'static str),
    Sub(&'static str, Vec<M>),
    Sep,
    /// planned item (shown disabled)
    #[allow(dead_code)]
    Todo(&'static str),
    /// submenu built by code ("Workspaces", "UI Elements")
    Dyn(&'static str),
}
use M::*;

fn poly_prims() -> Vec<M> {
    GEN_INFO.iter().filter(|g| !g.nurbs).map(|g| A(g.ty)).collect()
}
fn nurbs_prims() -> Vec<M> {
    let mut v: Vec<M> = GEN_INFO.iter().filter(|g| g.nurbs).map(|g| A(g.ty)).collect();
    v.push(A("nurbsCircle"));
    v.push(A("nurbsSquare"));
    v
}

pub fn common(name: &str) -> Vec<M> {
    match name {
        "File" => vec![A("newScene"), A("openScene"), A("saveScene"), A("saveSceneAs"), A("incrementAndSave"), A("revertScene"), Sep, A("importFile"), A("exportAll"), A("exportSelection"), Sep, A("projectWindow"), A("setProject"), Sep, Dyn("Recent Files"), Dyn("Recent Projects"), Sep, A("quit")],
        "Edit" => vec![
            A("undo"),
            A("redo"),
            A("repeatLast"),
            Sep,
            A("cut"),
            A("copy"),
            A("paste"),
            Sep,
            Sub("Keys", vec![A("copyKeys"), A("pasteKeys"), A("deleteKeysCurrent"), A("deleteChannels"), A("bakeKeys")]),
            Sep,
            A("deleteSel"),
            Sub("Delete by Type", vec![A("deleteHistory"), A("deleteChannels"), A("deleteAllLights"), A("deleteAllCameras"), A("deleteAllCurves")]),
            Sub("Delete All by Type", vec![A("deleteAllHistory"), A("deleteAllLights"), A("deleteAllCameras"), A("deleteAllCurves")]),
            Sep,
            A("duplicate"),
            A("duplicateSpecial"),
            A("instance"),
            A("duplicateSpecialOptions"),
            A("duplicateWithTransform"),
            A("duplicateHistory"),
            Sep,
            A("group"),
            A("ungroup"),
            Sep,
            A("parent"),
            A("unparent"),
            Sep,
            A("searchReplace"),
            A("prefixHierarchy"),
            A("renameSel"),
        ],
        "Create" => vec![
            Sub("Polygon Primitives", poly_prims()),
            Sub("NURBS Primitives", nurbs_prims()),
            Sep,
            Sub("Lights", LIGHT_TYPES.iter().map(|l| A(l.0)).collect()),
            Sub("Cameras", vec![A("createCamera"), A("createCameraAim")]),
            Sep,
            Sub("Curve Tools", vec![A("cvCurveTool"), A("epCurveTool"), A("pencilCurveTool"), A("arcTool")]),
            A("createPolygonTool"),
            Sep,
            A("createType"),
            A("createEmptyGroup"),
            A("createLocator"),
            Sep,
            Sub("Measure Tools", vec![A("distanceTool")]),
        ],
        "Select" => vec![
            A("selectAll"),
            Sub("All by Type", vec![A("selectAllGeometry"), A("selectAllLights"), A("selectAllCameras"), A("selectAllCurves")]),
            A("deselectAll"),
            A("selectHierarchy"),
            A("invertSelection"),
            A("selectSimilar"),
            Sep,
            A("growSelection"),
            A("shrinkSelection"),
            Sep,
            A("objectMode"),
            A("vertexMode"),
            A("edgeMode"),
            A("faceMode"),
            A("uvMode"),
            Sep,
            A("selectEdgeLoop"),
            A("selectEdgeRing"),
            A("selectBorder"),
            Sep,
            Sub("Convert Selection", vec![A("toVertices"), A("toEdges"), A("toFaces")]),
            A("selectByName"),
        ],
        "Modify" => vec![
            Sub("Transformation Tools", vec![A("moveTool"), A("rotateTool"), A("scaleTool"), A("pivotEdit")]),
            A("resetTransformations"),
            A("freezeTransformations"),
            Sub("Match Transformations", vec![A("matchTranslation"), A("matchRotation"), A("matchAll")]),
            Sep,
            Sub("Pivot", vec![A("centerPivot"), A("pivotEdit")]),
            A("centerPivot"),
            A("snapToGround"),
            Sep,
            A("prefixHierarchy"),
            A("searchReplace"),
            Sep,
            Sub("Convert", vec![A("convertNurbsToPoly")]),
            Sub("Asset", vec![A("createContainer"), A("publishAttributes")]),
            A("renameSel"),
            Sep,
            A("addAttribute"),
        ],
        "Display" => vec![
            A("toggleGrid"),
            Sub("Heads Up Display", vec![A("hudPolyCount"), A("hudFrameRate"), A("hudCurrentFrame"), A("hudCameraNames"), A("hudViewAxis")]),
            Dyn("UI Elements"),
            Sep,
            Sub("Hide", vec![A("hideSelection"), A("hideUnselected"), Sep, A("hideAllLights"), A("hideAllCameras"), A("hideAllGeometry")]),
            Sub("Show", vec![A("showSelection"), A("showLastHidden"), A("showAll"), Sep, A("showAllLights"), A("showAllCameras"), A("showAllGeometry")]),
            Sub("Object Display", vec![A("templateSel"), A("untemplateSel"), A("untemplateAll")]),
            A("toggleWireOnShaded"),
            A("toggleXray"),
            A("toggleBackface"),
            A("cycleBackground"),
            Sep,
            Sub("Polygons", vec![A("faceCenters"), Sep, A("smoothOff"), A("smoothCage"), A("smoothOn"), Sep, A("toggleNormals"), A("toggleBorderEdges"), A("softenEdge"), A("hardenEdge")]),
            A("frameAll"),
            A("frameSelected"),
            A("frameAllAll"),
            A("frameSelectedAll"),
            A("isolateSelect"),
        ],
        "Windows" => vec![
            Dyn("Workspaces"),
            Sep,
            Sub("General Editors", vec![A("scriptEditor"), A("componentEditor"), A("connectionEditor"), A("hypergraph"), A("hypergraphConnections"), A("shelfEditor"), A("melReference")]),
            A("modelingToolkit"),
            Sub("Modeling Editors", vec![A("uvEditor")]),
            Sub("Rendering Editors", vec![A("hypershade"), A("renderView"), A("renderSettings"), A("playblast")]),
            Sub("Animation Editors", vec![A("graphEditor"), A("dopeSheet")]),
            Sep,
            A("outlinerWindow"),
            A("attributeEditor"),
            A("channelBox"),
            A("toolSettings"),
            Sep,
            Sub("Settings/Preferences", vec![A("preferences"), A("hotkeyEditor"), A("shelfEditor")]),
        ],
        "Help" => vec![A("gettingStarted"), A("hotkeyList"), A("mouseHelp"), Sep, A("melReference"), Sep, A("about")],
        _ => vec![],
    }
}

pub fn set_menus(ms: MenuSet) -> Vec<(&'static str, Vec<M>)> {
    match ms {
        MenuSet::Modeling => vec![
            ("Mesh", vec![Sub("Booleans", vec![A("booleanUnion"), A("booleanDifference"), A("booleanIntersection")]), A("combine"), A("separate"), Sep, A("fillHole"), A("smooth"), A("triangulate"), A("quadrangulate"), Sep, A("mirror"), Sep, A("cleanup")]),
            ("Edit Mesh", vec![A("addDivisions"), A("bevel"), A("bridge"), A("collapse"), A("connect"), A("chamferVertex"), A("extrude"), A("merge"), A("mergeToCenter"), A("poke"), Sep, A("deleteEdgeVertex"), Sep, A("extract"), A("duplicateFaces"), Sep, A("curveFromEdges")]),
            ("Mesh Tools", vec![A("createPolygonTool"), A("insertEdgeLoopTool"), A("multiCutTool"), A("quadDrawTool"), A("targetWeldTool"), A("sculptTool"), Sub("Sculpt Brushes", vec![A("smoothSculptTool"), A("relaxSculptTool"), A("grabSculptTool"), A("pinchSculptTool"), A("flattenSculptTool"), A("inflateSculptTool")]), A("makeLive"), Sep, A("insertEdgeLoop")]),
            ("Mesh Display", vec![A("softenEdge"), A("hardenEdge"), A("softHardAngle"), Sep, A("reverseNormals"), Sep, A("toggleNormals"), A("toggleBorderEdges"), Sep, A("smoothOff"), A("smoothOn")]),
            ("UV", vec![A("uvEditor"), Sep, A("planarMap"), A("cylindricalMap"), A("sphericalMap"), A("automaticMap")]),
            ("Surfaces", vec![A("revolve"), A("loft"), A("extrudeCurve"), A("planarSurface")]),
            ("Curves", vec![A("cvCurveTool"), A("epCurveTool"), A("pencilCurveTool"), Sep, A("reverseCurve"), A("rebuildCurve"), A("openCloseCurve"), Sep, A("curveFromEdges")]),
        ],
        MenuSet::Rigging => vec![
            ("Skeleton", vec![A("jointTool"), A("ikHandleTool"), Sep, A("insertJoint"), A("mirrorJoint"), A("orientJoint"), Sep, A("humanIK"), Sub("HumanIK", vec![A("hikCreateSkeleton"), A("hikDefineCharacter"), A("hikCreateControlRig"), Sep, A("hikBakeCharacter")])]),
            ("Skin", vec![A("smoothBind"), A("unbindSkin"), Sep, A("paintWeights")]),
            ("Deform", vec![Sub("Nonlinear", vec![A("bendDeformer"), A("flareDeformer"), A("sineDeformer"), A("squashDeformer"), A("twistDeformer"), A("waveDeformer")]), Sep, A("createCluster"), A("softModDeformer"), A("createLattice"), A("latticePointTool"), A("blendShape"), Sep, A("deleteDeformers")]),
            ("Constrain", vec![A("parentConstraint"), A("pointConstraint"), A("orientConstraint"), A("scaleConstraint"), A("aimConstraint"), A("poleVectorConstraint"), Sep, A("removeConstraints"), Sep, A("attachToMotionPath")]),
            ("Control", vec![A("createLocator"), A("group"), Sep, A("setDrivenKeyWindow")]),
        ],
        MenuSet::Animation => vec![
            ("Key", vec![A("setKey"), A("keyTranslate"), A("keyRotate"), A("keyScale"), Sep, A("setDrivenKeyWindow"), Sep, A("copyKeys"), A("pasteKeys"), A("deleteKeysCurrent"), A("bakeKeys"), Sep, A("toggleAutoKey")]),
            ("Playback", vec![A("playToggle"), A("playBackwards"), A("stopPlayback"), Sep, A("nextFrame"), A("prevFrame"), A("nextKey"), A("prevKey"), A("goToStart"), A("goToEnd"), Sep, A("playblast")]),
            ("Visualize", vec![A("graphEditor"), A("dopeSheet"), Sep, A("createGhost"), A("unghostAll"), A("motionTrail")]),
        ],
        MenuSet::Fx => vec![
            ("Particles", vec![A("createEmitter"), A("createDirectionalEmitter"), A("createParticles"), A("emitFromObject")]),
            ("Fields/Solvers", vec![A("gravityField"), A("dragField"), A("turbulenceField"), A("radialField"), A("vortexField"), A("airField"), Sep, A("affectSelected"), Sep, A("createActiveRigidBody"), A("createPassiveRigidBody"), A("removeRigidBody"), Sep, A("dynamicsReset")]),
            ("Effects", vec![A("createFire"), A("createSmoke")]),
        ],
        MenuSet::Rendering => vec![
            ("Lighting/Shading", {
                let mut v: Vec<M> = LIGHT_TYPES.iter().map(|l| A(l.0)).collect();
                v.extend([Sep, A("hypershade"), A("assignNewMaterial"), Sub("Assign New Material", vec![A("assignNew_lambert"), A("assignNew_blinn"), A("assignNew_phong"), A("assignNew_standardSurface"), A("assignNew_surfaceShader")]), Dyn("Assign Existing Material"), A("materialAttributes")]);
                v
            }),
            ("Texturing", vec![A("planarMap"), A("automaticMap"), A("uvEditor")]),
            ("Render", vec![A("renderCurrent"), A("ipr"), A("renderSequence"), Sep, A("renderView"), A("renderSettings"), A("playblast")]),
        ],
    }
}

/// Menu rows use Maya's blue selection fill, not the general gray button hover.
pub(crate) fn menu_hover(ui: &mut egui::Ui) {
    ui.style_mut().animation_time = 0.0;
    let v = &mut ui.style_mut().visuals.widgets;
    // Highlight only the row under the pointer. Keeping `active` blue made the row where
    // the press started stay highlighted while the pointer moved into a submenu.
    v.hovered.bg_fill = crate::theme::HI;
    v.hovered.weak_bg_fill = crate::theme::HI;
}

pub fn items(app: &mut IncaApp, ui: &mut egui::Ui, list: &[M]) {
    menu_hover(ui);
    for it in list {
        match it {
            A(id) => {
                let checked = toggle_state(app, id);
                let mut text = label(id);
                if let Some(c) = checked {
                    text = format!("{} {text}", if c { "✔" } else { "  " });
                }
                let b = egui::Button::new(text).shortcut_text(crate::hotkey_editor::chords_for(app, id).first().cloned().unwrap_or_default());
                if crate::options::has_box(id) {
                    // the option box opens the command's options dialog
                    let mut opened = false;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let main = ui.add(b);
                        highlight_under_press(ui, &main);
                        if main.clicked() || released_here(ui, &main) {
                            menu_pick(app, ui, id);
                            ui.close();
                        }
                        let opt = crate::icons::button_ex(ui, "optionBox", "Options", false, 16.0, 12.0, true);
                        highlight_under_press(ui, &opt);
                        if opt.clicked() || released_here(ui, &opt) {
                            opened = true;
                        }
                    });
                    if opened {
                        crate::options::open(app, id);
                        app.open_menu = None;
                        ui.close();
                    }
                } else {
                    let r = ui.add(b);
                    highlight_under_press(ui, &r);
                    if r.clicked() || released_here(ui, &r) {
                        menu_pick(app, ui, id);
                        ui.close();
                    }
                }
            }
            Sub(l, v) => {
                ui.menu_button(*l, |ui| items(app, ui, v));
            }
            Sep => {
                ui.separator();
            }
            Todo(l) => {
                ui.add_enabled(false, egui::Button::new(format!("{l} (planned)")));
            }
            Dyn(l) => {
                ui.menu_button(*l, |ui| match *l {
                    "Workspaces" => crate::workspace::menu(app, ui),
                    "Recent Files" | "Recent Projects" => {
                        let files = *l == "Recent Files";
                        let list = if files { app.recent.clone() } else { app.recent_projects.clone() };
                        if list.is_empty() {
                            ui.add_enabled(false, egui::Button::new("(none)"));
                        }
                        for r in list {
                            let resp = ui.button(&r);
                            highlight_under_press(ui, &resp);
                            if resp.clicked() || released_here(ui, &resp) {
                                if files {
                                    app.open_path(std::path::Path::new(&r));
                                } else {
                                    crate::project::set_project(app, &r, false);
                                }
                                app.open_menu = None;
                                ui.close();
                            }
                        }
                    }
                    "Assign Existing Material" => {
                        let mats: Vec<(String, String)> = app.s.mats.values().map(|m| (m.id.clone(), m.name.clone())).collect();
                        for (_, name) in mats {
                            let resp = ui.button(&name);
                            highlight_under_press(ui, &resp);
                            if resp.clicked() || released_here(ui, &resp) {
                                let _ = app.run_mel("assign material", &format!("hyperShade -assign {name};"));
                                app.open_menu = None;
                                ui.close();
                            }
                        }
                    }
                    _ => crate::workspace::elements_menu(app, ui),
                });
            }
        }
    }
}

/// True when the primary button was released over this item. The JS menus run items on
/// `mouseup`, so holding the button after opening a menu and releasing over an item runs it.
pub fn released_here(ui: &egui::Ui, r: &egui::Response) -> bool {
    r.contains_pointer() && ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
}

/// A menu item click: runs the command, or with Ctrl+Shift adds it to the current shelf (as in Maya).
fn menu_pick(app: &mut IncaApp, ui: &egui::Ui, id: &str) {
    app.open_menu = None;
    let m = ui.input(|i| i.modifiers);
    if (m.ctrl || m.command) && m.shift {
        crate::toolbar::add_to_shelf(app, id);
    } else {
        app.action(id);
    }
}

/// Checkmark state for toggle actions.
pub fn toggle_state(app: &IncaApp, id: &str) -> Option<bool> {
    let v = &app.vps[app.active_vp];
    Some(match id {
        "toggleGrid" => v.grid,
        "toggleHud" => v.hud,
        "hudPolyCount" => app.hud.poly_count,
        "hudFrameRate" => app.hud.fps,
        "hudCurrentFrame" => app.hud.frame,
        "hudCameraNames" => app.hud.cam_names,
        "hudViewAxis" => app.hud.view_axis,
        "toggleWireOnShaded" => v.wire_on_shaded,
        "faceCenters" => v.face_centers,
        "toggleXray" => v.xray,
        "toggleBackface" => app.backface,
        "toggleShelf" => app.shown("shelf"),
        "toggleTimeline" => app.shown("timeslider"),
        "toggleCmdline" => app.shown("cmdline"),
        "toggleToolbox" => app.shown("toolbox"),
        "outlinerWindow" => app.show_outliner,
        "toggleAutoKey" => app.s.time.auto_key,
        _ => return None,
    })
}

pub fn menu_bar(app: &mut IncaApp, ui: &mut egui::Ui) {
    let mut titles: Vec<(&'static str, egui::Response)> = Vec::new();
    let mut entries: Vec<(&'static str, Vec<M>)> = Vec::new();
    let open = open_title(app.open_menu, MenuSource::Main);
    egui::MenuBar::new().ui(ui, |ui| {
        for name in ["File", "Edit", "Create", "Select", "Modify", "Display", "Windows"] {
            let list = common(name);
            titles.push((name, title_button(ui, name, open == Some(name))));
            entries.push((name, list));
        }
        ui.separator();
        for (name, list) in set_menus(app.menu_set) {
            titles.push((name, title_button(ui, name, open == Some(name))));
            entries.push((name, list));
        }
        let help = common("Help");
        titles.push(("Help", title_button(ui, "Help", open == Some("Help"))));
        entries.push(("Help", help));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| crate::workspace::selector(app, ui));
    });

    update_open_menu(ui.ctx(), app, &titles, MenuSource::Main, true);
    if let Some(name) = open_title(app.open_menu, MenuSource::Main) {
        if let Some((_, resp)) = titles.iter().find(|(n, _)| *n == name) {
            if let Some((_, list)) = entries.iter().find(|(n, _)| *n == name) {
                let suppress = released_on_title(ui.ctx(), &titles);
                app.menu_popup_rect = show_menu_popup(app, resp, list, suppress);
                return;
            }
        }
        // the menu set changed; the open title no longer exists
        app.open_menu = None;
    }
    app.menu_popup_rect = None;
}

/// A menu bar owned by a panel (Outliner, Attribute Editor, viewport, Render View, UV Editor).
/// It behaves exactly like the main menu bar: press a title to open it, hover another title
/// while it is open to switch, and release the mouse over a row to run it.
pub fn panel_bar(
    app: &mut IncaApp,
    ui: &mut egui::Ui,
    source: MenuSource,
    titles: &[&'static str],
    small: bool,
    mut body: impl FnMut(&mut IncaApp, &mut egui::Ui, &str),
) {
    let open = open_title(app.open_menu, source);
    let mut responses: Vec<(&'static str, egui::Response)> = Vec::with_capacity(titles.len());
    ui.horizontal_wrapped(|ui| {
        for name in titles {
            let r = if small {
                ui.add(egui::Button::new(egui::RichText::new(*name).size(11.5)).selected(open == Some(*name)))
            } else {
                title_button(ui, name, open == Some(*name))
            };
            responses.push((*name, r));
        }
    });
    update_open_menu(ui.ctx(), app, &responses, source, true);
    let Some(name) = open_title(app.open_menu, source) else {
        app.menu_popup_rect = None;
        return;
    };
    let Some((_, resp)) = responses.iter().find(|(n, _)| *n == name) else {
        app.open_menu = None;
        app.menu_popup_rect = None;
        return;
    };
    let suppress = released_on_title(ui.ctx(), &responses);
    app.menu_popup_rect = show_menu_popup_ui(app, resp, suppress, |app, ui| body(app, ui, name));
}

fn open_title(open: Option<OpenMenu>, source: MenuSource) -> Option<&'static str> {
    open.filter(|m| m.source == source).map(|m| m.title)
}

fn title_button(ui: &mut egui::Ui, name: &str, open: bool) -> egui::Response {
    ui.add(egui::Button::new(name).selected(open))
}

/// Maya/JS top-level menu behaviour: press a title to open it, press it again to close, and
/// while it is open just hovering another title switches menus.
fn next_open(
    source: MenuSource,
    current: Option<OpenMenu>,
    pressed_at: Option<egui::Pos2>,
    hover_at: Option<egui::Pos2>,
    titles: &[(&'static str, egui::Rect)],
    hover_switch: bool,
) -> Option<OpenMenu> {
    let cur = open_title(current, source);
    if let Some(pos) = pressed_at {
        if let Some((name, _)) = titles.iter().find(|(_, r)| r.contains(pos)) {
            return if cur == Some(*name) { None } else { Some(source.with(name)) };
        }
    }
    if hover_switch {
        if let (Some(cur), Some(pos)) = (cur, hover_at) {
            if let Some((name, _)) = titles.iter().find(|(_, r)| r.contains(pos)) {
                if *name != cur {
                    return Some(source.with(name));
                }
            }
        }
    }
    current
}

fn update_open_menu(ctx: &egui::Context, app: &mut IncaApp, titles: &[(&'static str, egui::Response)], source: MenuSource, hover_switch: bool) {
    let rects: Vec<(&'static str, egui::Rect)> = titles.iter().map(|(name, r)| (*name, r.rect)).collect();
    let (pressed_at, hover_at) = ctx.input(|i| {
        // `contains_pointer` respects egui's layer order: a menu popup drawn above a panel's
        // menu bar must keep clicks from reaching the title underneath. Raw rect tests made a
        // press on a submenu item (e.g. Display > Heads Up Display > Camera Names) also count
        // as a press on any panel title it happened to overlap (Show / Render).
        let over_title = |pos: Option<egui::Pos2>| pos.is_some_and(|p| titles.iter().any(|(_, r)| r.contains_pointer() && r.rect.contains(p)));
        let press_pos = i.pointer.interact_pos();
        let hover_pos = i.pointer.hover_pos();
        let pressed = i.pointer.button_pressed(egui::PointerButton::Primary) && over_title(press_pos);
        (if pressed { press_pos } else { None }, hover_pos.filter(|p| over_title(Some(*p))))
    });
    app.open_menu = next_open(source, app.open_menu, pressed_at, hover_at, &rects, hover_switch);
}

fn released_on_title(ctx: &egui::Context, titles: &[(&'static str, egui::Response)]) -> bool {
    ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
        && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| titles.iter().any(|(_, r)| r.rect.contains(pos)))
}

/// Draw the open menu below its title. `open_bool` gives us egui's click-outside and Escape
/// handling; the title-release case is suppressed because opening on press means the release
/// of that same click would otherwise close the new menu immediately.
fn show_menu_popup(app: &mut IncaApp, response: &egui::Response, list: &[M], suppress_close: bool) -> Option<egui::Rect> {
    show_menu_popup_ui(app, response, suppress_close, |app, ui| items(app, ui, list))
}

fn show_menu_popup_ui(
    app: &mut IncaApp,
    response: &egui::Response,
    suppress_close: bool,
    body: impl FnOnce(&mut IncaApp, &mut egui::Ui),
) -> Option<egui::Rect> {
    // Don't repaint continuously while a menu is open (that runs the whole window at
    // vsync). One delayed frame after pointer movement is enough to settle hover/submenu
    // state without leaving the previous row highlighted.
    if response.ctx.input(|i| i.pointer.is_moving()) {
        response.ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
    let mut open = true;
    let popup = egui::Popup::menu(response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let popup = if suppress_close { popup.open(true) } else { popup.open_bool(&mut open) };
    let shown = popup.show(|ui| {
        menu_hover(ui);
        body(app, ui);
    });
    if !open {
        app.open_menu = None;
    }
    shown.map(|r| r.response.rect)
}

/// True when a row was clicked, or when the button was released over it. The JS menus run rows
/// on `mouseup`, so holding the button after opening a menu and releasing over a row runs it.
/// Closes the open menu, as every JS menu row does.
pub fn highlight_under_press(ui: &egui::Ui, r: &egui::Response) {
    // While the button is held after opening a menu, the pressed title owns the interaction, so
    // egui reports the row as `contains_pointer` but not `hovered`. Highlight it ourselves to
    // show which row a release will run (Maya highlights it during the drag).
    if r.contains_pointer() && ui.input(|i| i.pointer.primary_down()) {
        r.clone().highlight();
    }
}

pub fn activated(app: &mut IncaApp, ui: &egui::Ui, r: &egui::Response) -> bool {
    highlight_under_press(ui, r);
    if r.clicked() || released_here(ui, r) {
        app.open_menu = None;
        true
    } else {
        false
    }
}

/// A plain menu row.
pub fn menu_item(app: &mut IncaApp, ui: &mut egui::Ui, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.button(label);
    activated(app, ui, &r)
}

/// A checkable menu row; like the JS menus it closes the popup when toggled.
pub fn menu_check(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.add(egui::Button::new(format!("{} {label}", if on { "✔" } else { "  " })));
    activated(app, ui, &r)
}

/// A radio-style menu row; closes the popup when picked. Uses the same check mark as the
/// checkable rows because the default egui fonts have no U+25CF (a missing-glyph box is shown).
pub fn menu_radio(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    let r = ui.add(egui::Button::new(format!("{} {label}", if on { "✔" } else { "  " })));
    activated(app, ui, &r)
}

/// A highlighted menu row for the current choice; closes the popup when picked.
pub fn menu_selectable(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.selectable_label(on, label);
    activated(app, ui, &r)
}

/// Maya's hotbox: every menu around the cursor while Space is held (stays until a click outside or Esc).
pub fn hotbox(app: &mut IncaApp, ctx: &egui::Context) {
    let Some(pos) = app.hotbox else { return };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.hotbox = None;
        app.open_menu = None;
        return;
    }
    let mut titles: Vec<(&'static str, egui::Response)> = Vec::new();
    let mut entries: Vec<(&'static str, Vec<M>)> = Vec::new();
    let open = open_title(app.open_menu, MenuSource::Hotbox);
    let r = egui::Area::new(egui::Id::new("hotbox")).fixed_pos(pos - egui::vec2(260.0, 70.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                for m in MenuSet::ALL {
                    if ui.selectable_label(app.menu_set == m, m.label()).clicked() {
                        app.menu_set = m;
                    }
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(520.0);
                for name in ["File", "Edit", "Create", "Select", "Modify", "Display", "Windows"] {
                    let list = common(name);
                    titles.push((name, title_button(ui, name, open == Some(name))));
                    entries.push((name, list));
                }
                for (name, list) in set_menus(app.menu_set) {
                    titles.push((name, title_button(ui, name, open == Some(name))));
                    entries.push((name, list));
                }
                let help = common("Help");
                titles.push(("Help", title_button(ui, "Help", open == Some("Help"))));
                entries.push(("Help", help));
            });
        });
    });

    // The JS hotbox opens menus on press but (unlike the main menu bar) does not switch on hover.
    update_open_menu(ctx, app, &titles, MenuSource::Hotbox, false);
    if let Some(name) = open_title(app.open_menu, MenuSource::Hotbox) {
        if let Some((_, resp)) = titles.iter().find(|(n, _)| *n == name) {
            if let Some((_, list)) = entries.iter().find(|(n, _)| *n == name) {
                let suppress = released_on_title(ctx, &titles);
                app.menu_popup_rect = show_menu_popup(app, resp, list, suppress);
            } else {
                app.open_menu = None;
            }
        } else {
            app.open_menu = None;
        }
    }
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let in_menu = app.menu_popup_rect.is_some_and(|rect| pointer.is_some_and(|p| rect.contains(p)));
    let clicked_outside = ctx.input(|i| i.pointer.any_click()) && !r.response.contains_pointer() && !in_menu && !ctx.any_popup_open();
    if clicked_outside && app.space_down.is_none() {
        app.hotbox = None;
        if app.open_menu.is_some_and(|m| m.source == MenuSource::Hotbox) {
            app.open_menu = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    fn titles() -> Vec<(&'static str, Rect)> {
        ["File", "Edit", "Create"]
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, Rect::from_min_size(pos2(i as f32 * 30.0, 0.0), vec2(30.0, 20.0))))
            .collect()
    }

    #[test]
    fn hover_switches_an_open_menu() {
        let r = titles();
        let open = next_open(MenuSource::Main, Some(MenuSource::Main.with("File")), None, Some(pos2(40.0, 10.0)), &r, true);
        assert_eq!(open, Some(MenuSource::Main.with("Edit")));
    }

    #[test]
    fn pressing_the_open_title_closes_it() {
        let r = titles();
        let open = next_open(MenuSource::Main, Some(MenuSource::Main.with("File")), Some(pos2(10.0, 10.0)), None, &r, true);
        assert_eq!(open, None);
    }

    #[test]
    fn pressing_another_title_switches_without_hover() {
        let r = titles();
        let open = next_open(MenuSource::Main, Some(MenuSource::Main.with("File")), Some(pos2(70.0, 10.0)), None, &r, true);
        assert_eq!(open, Some(MenuSource::Main.with("Create")));
    }

    #[test]
    fn hovering_does_not_open_a_menu_by_itself() {
        let r = titles();
        let open = next_open(MenuSource::Main, None, None, Some(pos2(40.0, 10.0)), &r, true);
        assert_eq!(open, None);
    }

    #[test]
    fn the_hotbox_does_not_switch_on_hover() {
        let r = titles();
        let open = next_open(MenuSource::Hotbox, Some(MenuSource::Hotbox.with("File")), None, Some(pos2(40.0, 10.0)), &r, false);
        assert_eq!(open, Some(MenuSource::Hotbox.with("File")));
    }

    #[test]
    fn pressing_outside_keeps_the_open_menu() {
        let r = titles();
        let open = next_open(MenuSource::Main, Some(MenuSource::Main.with("File")), Some(pos2(200.0, 200.0)), None, &r, true);
        assert_eq!(open, Some(MenuSource::Main.with("File")));
    }

    #[test]
    fn one_bar_does_not_hover_switch_another_bars_menu() {
        let r = titles();
        let ae = MenuSource::AttrEditor.with("List");
        let open = next_open(MenuSource::Outliner, Some(ae), None, Some(pos2(40.0, 10.0)), &r, true);
        assert_eq!(open, Some(ae));
    }

    #[test]
    fn pressing_another_bars_title_takes_over() {
        let r = titles();
        let ae = MenuSource::AttrEditor.with("List");
        let open = next_open(MenuSource::Outliner, Some(ae), Some(pos2(40.0, 10.0)), None, &r, true);
        assert_eq!(open, Some(MenuSource::Outliner.with("Edit")));
    }
}
