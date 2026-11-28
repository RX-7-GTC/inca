//! Main menu bar with Maya menu sets (Modeling, Rigging, Animation, FX, Rendering).

use crate::actions::label;
use crate::app::{IncaApp, MenuSet};
use egui::{Align, Frame, InnerResponse, Layout, PointerButton, Popup, PopupCloseBehavior, RectAlign, Response, Ui, UiKind, UiStackInfo, Vec2};
use inca_core::primitives::GEN_INFO;
use inca_core::scene::LIGHT_TYPES;

/// Which menu bar an open menu belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuSource {
    Main,
    Hotbox,
    Outliner,
    AttrEditor,
    ChannelBox,
    LayerEditor,
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
            Sub("Panels", vec![A("panelShelf"), A("panelToolbox"), A("panelOutliner"), A("panelRight"), A("panelTimeslider"), A("panelRangeslider"), A("panelCmdline")]),
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
            ("Visualize", vec![A("graphEditor"), A("dopeSheet"), Sep, A("createGhost"), A("unghostSelected"), A("unghostAll"), A("toggleGhosts"), A("motionTrail")]),
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

/// Checkbox column used only in menus that contain checkable rows.
const MENU_PREFIX: &str = "            ";
const MENU_BAND_W: f32 = 24.0;

#[derive(Clone, Copy)]
struct MenuCheck {
    rect: egui::Rect,
    on: bool,
}

#[derive(Clone, Copy)]
struct MenuIcon {
    rect: egui::Rect,
    name: &'static str,
}

#[derive(Clone, Default)]
struct MenuChecks {
    checks: Vec<MenuCheck>,
    icons: Vec<MenuIcon>,
}

fn menu_checks_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("menu_checks")
}

fn menu_label(label: &str) -> String {
    format!("{MENU_PREFIX}{label}")
}

fn menu_begin(ui: &mut egui::Ui) {
    ui.data_mut(|d| d.insert_temp(menu_checks_id(ui), MenuChecks::default()));
}

fn menu_row_band(ui: &egui::Ui, rect: egui::Rect) {
    let band = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() - 1.0), egui::pos2(rect.left() + MENU_BAND_W, rect.bottom() + 1.0));
    ui.painter().rect_filled(band, 0.0, crate::theme::BG_DARK);
}

fn menu_record_check(ui: &mut egui::Ui, rect: egui::Rect, on: bool) {
    if ui.data(|d| d.get_temp::<MenuChecks>(menu_checks_id(ui)).is_some()) {
        ui.data_mut(|d| d.get_temp_mut_or_default::<MenuChecks>(menu_checks_id(ui)).checks.push(MenuCheck { rect, on }));
    } else {
        paint_menu_check(ui, rect, on);
    }
}

fn menu_record_icon(ui: &mut egui::Ui, rect: egui::Rect, name: &'static str) {
    if ui.data(|d| d.get_temp::<MenuChecks>(menu_checks_id(ui)).is_some()) {
        ui.data_mut(|d| d.get_temp_mut_or_default::<MenuChecks>(menu_checks_id(ui)).icons.push(MenuIcon { rect, name }));
    } else {
        paint_menu_icon(ui, rect, name);
    }
}

fn menu_end(ui: &mut egui::Ui) {
    let items = ui.data_mut(|d| d.remove_temp::<MenuChecks>(menu_checks_id(ui)).unwrap_or_default());
    let rect = ui.min_rect();
    let band = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top()), egui::pos2(rect.left() + MENU_BAND_W, rect.bottom()));
    ui.painter().rect_filled(band, 0.0, crate::theme::BG_DARK);
    for i in items.icons {
        paint_menu_icon(ui, i.rect, i.name);
    }
    for c in items.checks {
        paint_menu_check(ui, c.rect, c.on);
    }
}

fn paint_menu_check(ui: &egui::Ui, rect: egui::Rect, on: bool) {
    // Center the box on the same axis as the tool icons, and make it big enough to read at
    // the menu font size.
    let box_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 12.0, rect.center().y), egui::vec2(12.0, 12.0));
    let edge = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.painter().rect_stroke(box_rect, 1.0, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    if on {
        let color = ui.visuals().text_color();
        ui.painter().line_segment([egui::pos2(box_rect.left() + 2.5, rect.center().y), egui::pos2(box_rect.left() + 5.0, rect.center().y + 2.5)], egui::Stroke::new(1.4, color));
        ui.painter().line_segment([egui::pos2(box_rect.left() + 5.0, rect.center().y + 2.5), egui::pos2(box_rect.right() - 2.0, rect.center().y - 2.5)], egui::Stroke::new(1.4, color));
    }
}

fn paint_menu_icon(ui: &egui::Ui, rect: egui::Rect, name: &str) {
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 12.0, rect.center().y), egui::vec2(16.0, 16.0));
    crate::icons::paint(ui, icon_rect, name, egui::Color32::WHITE);
}

pub fn submenu_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let button = ui.add(egui::Button::new(menu_label(label)).right_text(egui::menu::SubMenuButton::RIGHT_ARROW));
    menu_row_band(ui, button.rect);
    button
}

fn paint_option_box(ui: &egui::Ui, rect: egui::Rect, hovered: bool) {
    let box_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(10.0, 10.0));
    let color = if hovered { egui::Color32::WHITE } else { egui::Color32::from_gray(0xc8) };
    ui.painter().rect_stroke(box_rect, 1.0, egui::Stroke::new(1.0, color), egui::StrokeKind::Inside);
}

fn items(app: &mut IncaApp, ui: &mut egui::Ui, list: &[M]) {
    menu_hover(ui);
    for it in list {
        match it {
            A(id) => {
                let checked = toggle_state(app, id);
                let text = menu_label(&label(id));
                let has_box = crate::options::has_box(id);
                let keys = crate::hotkey_editor::chords_for(app, id).first().cloned().unwrap_or_default();
                let b = if has_box {
                    egui::Button::new(text)
                        .shortcut_text(keys)
                        .right_text(egui::Atom::custom(egui::Id::new(("options_space", id)), egui::vec2(18.0, 0.0)))
                } else {
                    egui::Button::new(text).shortcut_text(keys)
                };
                if has_box {
                    // the option box opens the command's options dialog
                    let mut opened = false;
                    let main = ui.add(b);
                    menu_row_band(ui, main.rect);
                    if let Some(on) = checked {
                        menu_record_check(ui, main.rect, on);
                    } else if let Some(icon) = crate::icons::action_icon(id) {
                        menu_record_icon(ui, main.rect, icon);
                    }
                    highlight_under_press(ui, &main);
                    let opt_rect = egui::Rect::from_center_size(egui::pos2(main.rect.right() - 8.0, main.rect.center().y), egui::vec2(16.0, 16.0));
                    let opt = ui.interact(opt_rect, main.id.with("option_box"), egui::Sense::click()).on_hover_text("Options");
                    highlight_under_press(ui, &opt);
                    let opt_hovered = opt.hovered();
                    let opt_clicked = opt.clicked() || released_here(ui, &opt);
                    let col_rect = egui::Rect::from_min_max(egui::pos2(opt_rect.left(), main.rect.top()), egui::pos2(main.rect.right(), main.rect.bottom()));
                    if opt_hovered {
                        main.clone().highlight();
                        ui.painter().rect_filled(col_rect, 3.0, crate::theme::HI);
                    } else {
                        ui.painter().rect_filled(col_rect, 0.0, ui.visuals().window_fill());
                    }
                    paint_option_box(ui, opt_rect, opt_hovered);
                    if opt_clicked {
                        opened = true;
                    } else if main.clicked() || released_here(ui, &main) {
                        menu_pick(app, ui, id);
                        ui.close();
                    }
                    if opened {
                        crate::options::open(app, id);
                        app.open_menu = None;
                        ui.close();
                    }
                } else {
                    let r = ui.add(b);
                    menu_row_band(ui, r.rect);
                    if let Some(on) = checked {
                        menu_record_check(ui, r.rect, on);
                    } else if let Some(icon) = crate::icons::action_icon(id) {
                        menu_record_icon(ui, r.rect, icon);
                    }
                    highlight_under_press(ui, &r);
                    if r.clicked() || released_here(ui, &r) {
                        menu_pick(app, ui, id);
                        ui.close();
                    }
                }
            }
            Sub(l, v) => {
                let button = submenu_button(ui, l);
                if submenu_show(ui, &button, |ui| items(app, ui, v)).is_some() {
                    button.highlight();
                }
            }
            Sep => {
                let s = ui.separator();
                menu_row_band(ui, s.rect);
            }
            Todo(l) => {
                let r = ui.add_enabled(false, egui::Button::new(format!("{MENU_PREFIX}{l} (planned)")));
                menu_row_band(ui, r.rect);
            }
            Dyn(l) => {
                let button = submenu_button(ui, l);
                if submenu_show(ui, &button, |ui| dyn_menu(app, ui, l)).is_some() {
                    button.highlight();
                }
            }
        }
    }
}

fn dyn_menu(app: &mut IncaApp, ui: &mut egui::Ui, label: &str) {
    match label {
        "Workspaces" => crate::workspace::menu(app, ui),
        "Recent Files" | "Recent Projects" => {
            let files = label == "Recent Files";
            let list = if files { app.recent.clone() } else { app.recent_projects.clone() };
            if list.is_empty() {
                menu_disabled(ui, "(none)");
            }
            for r in list {
                let resp = ui.button(menu_label(&r));
                menu_row_band(ui, resp.rect);
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
            let mats: Vec<(String, String)> = app.s.materials().into_iter().map(|m| (m.id, m.name)).collect();
            for (_, name) in mats {
                let resp = ui.button(menu_label(&name));
                menu_row_band(ui, resp.rect);
                highlight_under_press(ui, &resp);
                if resp.clicked() || released_here(ui, &resp) {
                    let _ = app.run_mel("assign material", &format!("hyperShade -assign {name};"));
                    app.open_menu = None;
                    ui.close();
                }
            }
        }
        _ => crate::workspace::elements_menu(app, ui),
    }
}

/// Adapted from egui 0.36's `SubMenu::show`, but with a negative gap so the submenu content
/// touches the parent menu. egui hardcodes a few pixels of empty space between the two popups,
/// which lets clicks fall through to the UI behind.
fn submenu_show<R>(ui: &Ui, button_response: &Response, content: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
    let frame = Frame::menu(ui.style());
    let id = button_response.id.with("submenu");
    let (open_item, menu_id) = egui::menu::MenuState::from_ui(ui, |state, stack| (state.open_item, stack.id));
    let menu_config = egui::menu::MenuConfig::find(ui);
    let menu_root_response = ui.ctx().read_response(menu_id)?;
    let hover_pos = ui.ctx().pointer_hover_pos();
    let menu_rect = menu_root_response.rect - frame.total_margin();
    let is_hovering_menu = hover_pos.is_some_and(|pos| ui.ctx().layer_id_at(pos) == Some(menu_root_response.layer_id) && menu_rect.contains(pos));

    let is_any_open = open_item.is_some();
    let mut is_open = open_item == Some(id);
    let was_open = is_open;
    let mut set_open = None;
    let button_rect = button_response.rect.expand2(ui.style().spacing.item_spacing / 2.0);
    let is_hovered = hover_pos.is_some_and(|pos| button_rect.contains(pos));
    let clicked = button_response.clicked();
    let clicked_by_pointer = button_response.clicked_by(PointerButton::Primary);
    let clicked_by_keyboard_or_access = clicked && !clicked_by_pointer;

    if ui.is_enabled() && is_open && clicked_by_keyboard_or_access {
        set_open = Some(false);
        is_open = false;
    }
    let should_open = ui.is_enabled() && ((!was_open && clicked) || (is_hovered && !is_any_open));
    if should_open {
        set_open = Some(true);
        is_open = true;
        egui::menu::MenuState::from_id(ui.ctx(), menu_id, |state| state.open_item = None);
    }

    let mut response = button_response.clone();
    response.interact_rect = response.interact_rect.expand2(Vec2::new(0.0, frame.total_margin().sum().y / 2.0));
    let popup_response = Popup::from_response(&response)
        .id(id)
        .open(is_open)
        .align(RectAlign::RIGHT_START)
        .layout(Layout::top_down_justified(Align::Min))
        .gap(-(frame.total_margin().sum().x / 2.0))
        .style(menu_config.style.clone())
        .frame(frame)
        .close_behavior(PopupCloseBehavior::IgnoreClicks)
        .info(UiStackInfo::new(UiKind::Menu).with_tag_value(egui::menu::MenuConfig::MENU_CONFIG_TAG, menu_config.clone()))
        .show(|ui| {
            if button_response.clicked() || button_response.is_pointer_button_down_on() {
                ui.ctx().move_to_top(ui.layer_id());
            }
            menu_begin(ui);
            let out = content(ui);
            menu_end(ui);
            out
        });

    if let Some(popup_response) = &popup_response {
        let is_deepest_submenu = egui::menu::MenuState::is_deepest_open_sub_menu(ui.ctx(), id);
        let clicked_outside = is_deepest_submenu && popup_response.response.clicked_elsewhere() && menu_root_response.clicked_elsewhere();
        let submenu_button_clicked = button_response.clicked();
        let clicked_inside = is_deepest_submenu
            && !submenu_button_clicked
            && response.ctx.input(|i| i.pointer.any_click())
            && hover_pos.is_some_and(|pos| popup_response.response.interact_rect.contains(pos));
        let click_close = match menu_config.close_behavior {
            PopupCloseBehavior::CloseOnClick => clicked_outside || clicked_inside,
            PopupCloseBehavior::CloseOnClickOutside => clicked_outside,
            PopupCloseBehavior::IgnoreClicks => false,
        };
        if click_close {
            set_open = Some(false);
            ui.close();
        }
        let is_moving_towards_rect = ui.input(|i| i.pointer.is_moving_towards_rect(&popup_response.response.rect));
        if is_moving_towards_rect {
            ui.request_repaint();
        }
        let hovering_other_menu_entry = is_open
            && !is_hovered
            && !popup_response.response.contains_pointer()
            && !is_moving_towards_rect
            && is_hovering_menu;
        if popup_response.response.should_close() {
            ui.close();
        }
        if hovering_other_menu_entry {
            set_open = Some(false);
        }
        if ui.will_parent_close() {
            ui.data_mut(|data| data.remove_by_type::<egui::menu::MenuState>());
        }
    }
    if let Some(set_open) = set_open {
        egui::menu::MenuState::from_id(ui.ctx(), menu_id, |state| state.open_item = set_open.then_some(id));
    }
    popup_response
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
        "panelShelf" => app.detached.contains("shelf"),
        "panelToolbox" => app.detached.contains("toolbox"),
        "panelOutliner" => app.detached.contains("outliner"),
        "panelRight" => app.detached.contains("rightpanel"),
        "panelTimeslider" => app.detached.contains("timeslider"),
        "panelRangeslider" => app.detached.contains("rangeslider"),
        "panelCmdline" => app.detached.contains("cmdline"),
        "toggleAutoKey" => app.s.time.auto_key,
        "toggleGhosts" => !app.viz.ghosts_hidden,
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
                ui.scope(|ui| {
                    menu_hover(ui);
                    ui.add(egui::Button::new(egui::RichText::new(*name).size(11.5)).selected(open == Some(*name)))
                })
                .inner
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
    ui.scope(|ui| {
        menu_hover(ui);
        ui.add(egui::Button::new(name).selected(open))
    })
    .inner
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
    // A right-click while a menu is open can also start a viewport marking menu. Close this
    // menu and clear egui's submenu state so it cannot get stuck waiting for a primary click.
    if response.ctx.input(|i| i.pointer.secondary_clicked()) {
        app.open_menu = None;
        response.ctx.data_mut(|data| data.remove_by_type::<egui::menu::MenuState>());
    }
    // A primary press outside the menu dismisses it and must pass through to the panel or
    // viewport so a marquee/drag can start immediately. The old full-screen click shield ate
    // that press, so dragging in the viewport did nothing while the menu stayed open.
    if response.ctx.input(|i| i.pointer.primary_pressed()) {
        if let Some(p) = response.ctx.input(|i| i.pointer.interact_pos()) {
            let over_menu = response.ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground);
            if !over_menu && p.y > response.rect.bottom() {
                app.open_menu = None;
                return None;
            }
        }
    }
    let mut open = true;
    let popup = egui::Popup::menu(response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let popup = if suppress_close { popup.open(true) } else { popup.open_bool(&mut open) };
    let shown = popup.show(|ui| {
        menu_hover(ui);
        menu_begin(ui);
        body(app, ui);
        menu_end(ui);
    });
    if !open {
        app.open_menu = None;
    }
    shown.map(|r| r.response.rect)
}

/// A right-click menu with the same checkbox band as the menu bars.
pub fn show_context_menu(app: &mut IncaApp, response: &egui::Response, body: impl FnOnce(&mut IncaApp, &mut egui::Ui)) {
    egui::Popup::context_menu(response)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .show(|ui| {
            menu_hover(ui);
            menu_begin(ui);
            body(app, ui);
            menu_end(ui);
        });
}

/// A right-click menu for arbitrary content: the content is indented past the checkbox band,
/// which stays continuous down the whole popup.
pub fn show_plain_context_menu(response: &egui::Response, body: impl FnOnce(&mut egui::Ui)) {
    egui::Popup::context_menu(response).show(|ui| {
        menu_hover(ui);
        menu_begin(ui);
        ui.horizontal(|ui| {
            ui.add_space(MENU_BAND_W + 4.0);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| body(ui));
        });
        menu_end(ui);
    });
}

/// A submenu for arbitrary content, indented past the same checkbox band.
pub fn show_plain_submenu(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    let button = ui.add(egui::Button::new(label).right_text(egui::menu::SubMenuButton::RIGHT_ARROW));
    let opened = submenu_show(ui, &button, |ui| {
        menu_hover(ui);
        menu_begin(ui);
        ui.horizontal(|ui| {
            ui.add_space(MENU_BAND_W + 4.0);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| body(ui));
        });
        menu_end(ui);
    })
    .is_some();
    if opened {
        button.highlight();
    }
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
    let r = ui.button(menu_label(label));
    menu_row_band(ui, r.rect);
    activated(app, ui, &r)
}

/// A plain menu row with an optional shortcut, drawn dim when `enabled` is false.
pub fn menu_item_ext(app: &mut IncaApp, ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    menu_hover(ui);
    let b = egui::Button::new(menu_label(label)).shortcut_text(shortcut);
    let r = ui.add_enabled(enabled, b);
    menu_row_band(ui, r.rect);
    activated(app, ui, &r)
}

/// A submenu row. Uses the custom popup so the submenu gets the same checkbox band.
pub fn menu_submenu<R>(ui: &mut egui::Ui, label: &str, content: impl FnOnce(&mut egui::Ui) -> R) -> bool {
    menu_hover(ui);
    let button = submenu_button(ui, label);
    let opened = submenu_show(ui, &button, content).is_some();
    if opened {
        button.highlight();
    }
    opened
}

/// A disabled menu row. Its label still starts after the checkbox band.
pub fn menu_disabled(ui: &mut egui::Ui, label: &str) {
    let r = ui.add_enabled(false, egui::Button::new(menu_label(label)));
    menu_row_band(ui, r.rect);
}

/// A dim explanatory row inside a menu, aligned with the other rows.
pub fn menu_note(ui: &mut egui::Ui, label: &str) {
    // Buttons preserve the leading gutter spaces; labels trim them.
    let r = ui.add(egui::Button::new(egui::RichText::new(menu_label(label)).color(crate::theme::TEXT_DIM)).frame(false).sense(egui::Sense::hover()));
    menu_row_band(ui, r.rect);
}

/// A checkable menu row; like the JS menus it closes the popup when toggled.
pub fn menu_check(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.add(egui::Button::new(menu_label(label)));
    menu_row_band(ui, r.rect);
    menu_record_check(ui, r.rect, on);
    activated(app, ui, &r)
}

/// A radio-style menu row; closes the popup when picked. Uses the same check mark as the
/// checkable rows because the default egui fonts have no U+25CF (a missing-glyph box is shown).
pub fn menu_radio(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.add(egui::Button::new(menu_label(label)));
    menu_row_band(ui, r.rect);
    menu_record_check(ui, r.rect, on);
    activated(app, ui, &r)
}

/// A highlighted menu row for the current choice; closes the popup when picked.
pub fn menu_selectable(app: &mut IncaApp, ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    menu_hover(ui);
    let r = ui.selectable_label(on, menu_label(label));
    menu_row_band(ui, r.rect);
    if on {
        menu_record_check(ui, r.rect, true);
    }
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

    #[test]
    fn open_menu_does_not_eat_a_viewport_marquee() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = crate::app::IncaApp::new(&cc);
        let mut frame = eframe::Frame::_new_kittest();
        app.right_tab = crate::app::RightTab::ChannelBox;
        app.open_menu = Some(OpenMenu { source: MenuSource::ChannelBox, title: "Channels" });
        let mut run = |app: &mut crate::app::IncaApp, events: Vec<egui::Event>, t: f64| {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 1000.0)));
            raw.time = Some(t);
            raw.events = events;
            app.windows_state.window_seen_now.clear();
            let mut out = ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, &mut frame));
            out.textures_delta.clear();
            std::mem::swap(&mut app.windows_state.window_seen, &mut app.windows_state.window_seen_now);
        };
        for i in 0..3 {
            run(&mut app, vec![], i as f64 * 0.1);
        }
        assert!(app.open_menu.is_some(), "Channel Box menu did not open");
        let start = pos2(700.0, 400.0);
        run(&mut app, vec![egui::Event::PointerMoved(start), egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], 0.4);
        assert!(app.open_menu.is_none(), "viewport press did not dismiss the menu");
        let end = pos2(850.0, 500.0);
        run(&mut app, vec![egui::Event::PointerMoved(end)], 0.5);
        assert!(app.vp_state.marquee.is_some(), "viewport marquee was blocked by the menu");
        run(&mut app, vec![egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], 0.6);
    }

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
