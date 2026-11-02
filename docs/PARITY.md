# JS → Rust parity checklist

What the Electron app (0.1.12) had that the Rust app needed, worked top to bottom and ticked when
done. Kept as a record; the Electron app was removed from the tree in 0.2.0. Compiled from the JS command
registry, menus, UI modules and a 0.1.12 screenshot.

## A. Look & layout (what you see at start-up)

- [x] A1. Icon set: the JS SVG icons (shelf, status line, tool box, outliner, panel toolbar, menus) instead of hand-drawn vector icons / text buttons
- [x] A2. Shelves: same shelf tabs and buttons as `shelf.js` (Curves/Surfaces, Poly Modeling, Sculpting, Rigging, Animation, Rendering, FX, Custom), icon buttons with labels on hover
- [x] A3. Status line: same groups and icons — menu set, new/open/save, undo/redo, hierarchy/object/component mode, selection masks, snap toggles (grid/curve/point/live), history, render/IPR/render settings, input line (Absolute / Relative / Rename / Select) with "x y z or name", Symmetry dropdown, panel toggles on the right
- [x] A4. Tool box: Select, Lasso, Paint, Move, Rotate, Scale, Last Tool; Quick Layout buttons at the bottom (single, four, side by side, outliner + persp, …)
- [x] A5. Workspaces: dock layout remembered between sessions (0.1.12 restores Outliner left + Attribute Editor right from prefs), Workspace selector on the right of the menu bar, workspaces (Maya Classic, Modeling Standard/Expert, Sculpting, Animation, Rigging, Rendering, UV Editing)
- [x] A6. Viewport defaults: flat grey background (Alt+B cycles grey / black / dark / light / gradient, saved), JS grid colours and `grid.divisions` / `grid.major` subdivisions (`grid.spacing` is snapping-only, like the JS); UI body text already 12px like the JS CSS
- [x] A7. Viewport panel: menus View / Shading / Lighting / Show / Renderer / Panels with the JS items, including the Bounding Box shading item and a separate NURBS Surfaces show filter; icon toolbar row (camera attrs, bookmarks, grid, film/resolution gate, wireframe/shaded/textured/lights/shadows/x-ray/isolate, exposure + gamma fields, colour management dropdown)
- [x] A8. Outliner: its own menu bar (Edit / Display / Show / Help), type icons, defaultLightSet / defaultObjectSet, shapes / materials / sets display options
- [x] A9. Attribute Editor: List / Selected / Focus / Attributes / Show / Help menus, node field + Focus + lock, bottom buttons Select / Load Attributes / Copy Tab / Close, collapsible sections; the object tab has Display (visibility, template, display layer), Node Behavior (Selectable, which also hides the node from picking and selection like JS `visibleNodes` / `Sel.select`) and Extra Attributes (notes, saved in `extra`); mesh shape tab has the JS Mesh Info, Smooth Mesh (display level plus the global `smoothDivisions` preview levels), Normals Display (`softAngle`) and Render Stats (Casts / Receive Shadows stored in the node `extra`, X-Ray) sections
- [x] A10. Time / Range Slider: play backwards, step key buttons, frame field, "film (24 fps)" style fps names, character set / anim layer menus (placeholders), auto key + playback prefs buttons, shift-drag range edits
- [x] A11. Side tabs (vertical, toggle the right dock), dock widths remembered, Hide/Show UI elements (Ctrl+Space)

## B. Commands & menus missing in Rust

- [x] B1. File: Increment & Save, Revert to Saved, Project Window / Set Project, Recent Projects, autosave
- [x] B2. Edit: Cut, Copy/Paste Keys, Delete Keys (current frame), Bake Simulation, Delete All by Type (lights/cameras/curves), Duplicate Special (with options), Duplicate with Transform (Shift+D), Search and Replace Names, Prefix Hierarchy Names, Select Similar, Select by Name, All by Type (geometry/lights/cameras/curves)
- [x] B3. Modify: Match Translation / Rotation / All, Add Attribute, Prefix Hierarchy
- [x] B4. Display: HUD toggles (poly count, fps, frame, camera names, view axis), Hide Unselected / Show Selection / Show Last Hidden, Hide/Show lights/cameras/geometry, Template / Untemplate / Untemplate All, Face Normals, Border Edges, Display ▸ Polygons ▸ Face Centers (off by default, like Maya), Toggle Background, Frame Selection/All in all views
- [x] B5. Mesh: Cleanup, Collapse, Connect, Chamfer Vertices, Polygon Edges to Curve, Soften/Harden (30°) (Reduce / Conform / Conform Normals are menu-only placeholders in the JS too)
- [x] B6. Mesh Tools: Insert Edge Loop tool (interactive), Multi-Cut, Target Weld, Quad Draw, Sculpt tool (brushes), Make Live
- [x] B7. Curves / Surfaces: Pencil Curve tool, Open/Close (Attach / Detach / Birail / Boundary are menu-only placeholders in the JS too)
- [x] B8. UV: UV Editor (select / move / rotate / scale UVs, flip, rotate 90, normalize, layout, unfold/relax, shells, checker / shading)
- [x] B8b. UV component mode in the viewport (F12) and the texture image behind the UV Editor
- [x] B9. Selection: Lasso and Paint tools, Soft Select (B, B+drag radius, falloff colours, smooth / linear), Symmetry (object X, world X/Y/Z), cycle component type (Shift+Q), pre-selection highlight
- [x] B10. Manipulators: hold V point snap, hold X grid snap, hold J discrete rotate, W/E/R + LMB marking menus, manipulator size (+/-) readout + reset; per-tool Discrete Move/Rotate/Scale toggles and steps as JS `Manip.discrete` (grid snap is separate and uses the grid spacing); `snapDuringDrag` point snapping (mesh verts + curve CVs, no radius cap, selected objects excluded in object mode, moved components skipped, projected onto the dragged axis/plane, point snap beats the grid); centre-handle grid snap on the ground plane (persp) or view plane (ortho) preserving the grab offset
- [x] B10b. Ctrl+click axis lock and MMB drag along the active handle (needs a custom manipulator instead of transform-gizmo)
- [x] B11. Marking menus: RMB object/component menus, Shift+RMB tool menus, Ctrl+RMB conversion; In-View Editor for new history nodes; option boxes (smooth, mirror, add divisions, extrude, bridge, bevel, merge, planar map, revolve, group, chamfer, duplicate special)
- [x] B12. Rigging: IK handles (RP / SC, pole vector, twist), Insert / Mirror / Orient Joint, Unbind Skin, Paint Skin Weights, Cluster, Lattice (+ point tool), Soft Modification, Blend Shape, Delete Deformers, Pole Vector constraint, Remove Constraints, Set Driven Key window; load the JS rig nodes (joints, IK, lattices, constraints) instead of keeping them as locators
- [x] B13. Animation: (Play Backwards, Stop, Copy/Paste Keys done) Bake, Ghosting, Motion Trails, Playblast (video / PNG sequence)
- [x] B14. FX: Create Particles (particle tool), Rewind Simulation, Remove Rigid Body, per-field connections
- [x] B15. Rendering: IPR, Render Sequence (batch to the project images folder), keep/compare images, exposure / gamma / alpha display in Render View, Hardware renderer option, Assign New Material dialog, Material Attributes
- [x] B16. Editors: Hypergraph (hierarchy + input/output connections), Connection Editor, Shelf Editor, Hotkey Editor (editable keymap), Modeling Toolkit panel, Component Editor, Preferences window with all JS sections (interface, display, manipulators, animation, time slider, units, undo, files, autosave)
- [x] B17. Hypershade node graph work area (drag to connect), swatch renders
- [x] B18. Help: Getting Started, Mouse & Navigation
- [x] B19. Create: Type (3D text), Distance Tool, Camera and Aim
- [x] B20. MEL: `connectionInfo`, `distanceDimension`, `instance`, `inViewMessage`, `python`/`echo` stubs; JavaScript `cmds.*` tab equivalent stays Rhai

## C. Behaviour differences to check

- [x] C1. Hotkeys: compare every JS hotkey (F2–F6 menu sets, Alt+B, Ctrl+Space, B, V, J, Shift+D, Ctrl+Shift+click to shelf, …)
- [x] C2. Preferences persisted to a prefs file (JS stores menu set, workspace, HUD, shelves, hotkeys)
- [x] C3. Viewport visuals: pre-selection highlight, component colours, wire colours, selected-face colour as in JS; component-mode edges keep the JS hilite blue and a hilited-but-unselected curve uses the hilite wire colour; HUD frame shows two decimals (JS `fmt(frame, 2)`); resolution/film gate masks, borders and labels match `drawGate`; Frame Selection frames selected components and falls back to Frame All when its box is empty, and an empty frame uses the JS ±6 box and the JS orthographic aspect factor; Frame All only unions visible mesh/curve/light/locator/camera objects and never the startup cameras (JS `frameAll`); shaded materials use the JS `polygonOffset(1, 1)` so depth-tested wires sit on their surface without a clip-space bias that would reveal hidden back edges at the default camera distance
- [x] C4. Channel Box: MMB virtual slider in viewport, Ctrl/MMB drag in fields, expressions (`+=2`), int/enum stepping
- [x] C5. Component selection: switching vertex/edge/face converts the selection with the JS `convertComp` fallbacks (lone vertex → incident edges / faces), and per-object component sets survive a trip through object mode (JS `Sel.setMode`); the component hilite is a separate list from the object selection (`App.hilite`), so object selection changes in component mode leave the edited objects alone, clicking another object hilites it and appends it to the selection, and leaving component mode keeps the hilited objects selected. Undo snapshots component mode, the hilite and the component sets (JS `Undo.snapshot`).
- [x] C6. Camera navigation and framing follow the JS: tumble accumulates the stored Euler angles directly (`r[1] -= dx*0.4`, `r[0] -= dy*0.4`, no pole clamp, rotate order preserved), bookmarks/default views do not reset the rotate order, orthographic Frame parks the camera 1000 units back with `coi = 1000`, Look at Selection also works in orthographic views and unions only selected object bounding boxes, and new cameras start at (0, 2, 10) like JS `createCamera`.
- [x] C7. Viewport icons: light and user-camera wireframes use the JS `lightIcon` / `cameraIcon` geometry (point/directional/spot/area/ambient/sky dome, camera body + lens + reels) and base `WIRE_MATS.icon` near-black (locators `0x10106e`); selection, lead, affected and template colours replace the base colour exactly as `wireMat` does.
- [x] C8. Shading: per-object X-Ray (Attribute Editor ▸ Render Stats, saved in scene JSON and undo, `setAttr xray`) blends at `min(opacity, 0.5)` with depth write off like JS `prepareScene`; backface culling is one global `App.backfaceCulling` switch mirrored into every pane (toggle, menus, toolbar and picking); default lighting uses the JS `defaultLight` surface-to-light vector `normalize(-0.15*right + 0.2*up - cam_dir)`; each object's x-ray affects rendering only, not `occluded()`.
- [x] C9. Sky dome lights: with Use All Lights the last visible `skyDomeLight` becomes the environment and, when `showBackground` is on, the viewport background is an equirect sampled by the view ray (zenith/horizon/ground, exposure and sRGB like the JS); it is excluded from the punctual light list. `textureFile` decodes HDR/EXR (linear) and PNG/JPG (sRGB, like `loadSkyTexture`), applies the light colour tint, and uses the image for both the background and a CPU-prefiltered diffuse irradiance map (the PMREM roughness-1 term `scene.environment` feeds to diffuse shading). While the file is missing or fails to decode the JS `proceduralSky` fallback is used and a warning is printed; the Attribute Editor file field has a browse button like the JS `type: 'file'` row. The active panel's own camera icon is not drawn, like `n !== this.camNode` (startup cameras were already hidden). Deviations: loading is synchronous (the JS loads in the background), the image is capped at 2048 wide, and there is no PMREM specular environment (roughness-filtered reflections) — only diffuse irradiance, like the previous procedural approximation.
- [x] C10. Hardware Texturing: `Shading > Hardware Texturing` (6) samples the material colour map per fragment from a 64-layer texture array (procedural checker/grid/ramp/noise rendered like `proceduralCanvas`, file textures loaded from the project or an absolute path and resized, sRGB like the JS `colorSpace`), with the JS UV transform (`repeatU/V`, `offsetU/V`, `rotateUV`) and `white * base/diffuse` tint; shaded mode draws the flat `matsU` colour instead of the texture's average tone, and per-face materials pick their own map. Deviations: only the colour map is sampled (bump, roughness, metalness, emissive and opacity maps are still flat), textures beyond the 64 layers fall back to flat colour, and the texture array has no mipmaps (linear minification).
- [x] C11. Shadows: `Lighting > Shadows` and the toolbar toggle enable depth-map shadows only with Use All Lights, matching `renderer.shadowMap.enabled = o.shadows && o.lighting === 'all'`; per-light `shadows` / `shadowRadius` (default 2) and per-mesh Render Stats Casts / Receive Shadows (default on) are saved in the scene. Directional lights use the JS ±25 orthographic shadow camera, spots use `fov = 2 * angle`, point lights render six 90° cube faces, and sampling uses the three.js bias / normalBias with a comparison-sampled PCF kernel (the JS `shadowRadius` is rescaled to the 512² map so the world-space softness matches); the hardware renderer enables shadows like the JS render view. Deviations: one 512² × 48-layer depth array is shared by all lights (the JS viewport's shadow maps are also 512² by default, but its hardware renderer raises them to `hwShadowMap`, 2048²), and sampling uses a 3×3 comparison PCF kernel instead of three r186's five-tap Vogel disk. Area lights cast no shadows, which matches three.js (RectAreaLight has no shadow camera).
- [x] C12. Menu interaction: every menu bar (main, Outliner, Attribute Editor, viewport panels, Render View, UV Editor) opens on mouse press, follows hover between its own top-level titles while open, highlights the row under the cursor, and runs that row when the button is released, like JS 0.1.12 and Maya; the hotbox keeps its press-open behavior and deliberately does not hover-switch.

## Closed GitHub issues re-checked against the Rust version

| Issue | Status in Rust |
|---|---|
| #1 ghost keyframes from Channel Box clicks | ok: rows only show key colours for real anim curves |
| #2 backface culling | fixed (front faces were culled, back edges drawn) |
| #3 extrude local translate | fixed: Move after Extrude drives the extrude node, divisions follow |
| #4 integer channels as floats | fixed: virtual slider steps whole numbers within range (INPUTS too) |
| #5 Multi-Cut zoom commits | ok |
| #6 Multi-Cut snaps to own points | ok |
| #7 G repeat | fixed: tool completion / view / selection commands no longer replace the repeat |
| #8 Outliner Ctrl selection | ok |
| #9 gizmo moves during Alt navigation | ok |
| #10 face highlight follows pointer | ok |
| #11 In-View Editor movable | fixed: drag its title |
| #12 one value into several picked channels | fixed |
| #13 hotbox menus open on hover | ok: hotbox menus open on press, toggle on a second press, and run the item under the cursor on release like JS 0.1.12 (which does not hover-switch) |
| #14 / #15 status line and shelf icons | ok |
| #16 Channel Box channels drive the manipulator | fixed: picked transform channels show their axes, MMB drags them |
| #17 W/E/R + LMB marking menu | ok |
| #18 Edit Pivot vertex snap | ok |
| #19 Edit Pivot / centre handle grid snap | fixed: centre handle snaps on the ground plane; D is a hold key, Insert toggles |
| #20 Hypergraph InputOutput multiple objects | ok |
| #21 Connection Editor | ok |
| #27 double-click border edge selects rim | fixed (double-click edge loop was missing; loops now follow open borders) |

## Electron pull requests and open issues ported to Rust

| PR / issue | Rust version |
|---|---|
| #22 Three Point Arc editing | drag points / centre, click to flip, radius readout |
| #23 Edit Pivot rings | blue ring, inner ring while point snapping |
| #24 Hypergraph parallel connections | one line per connection, coloured by attribute type, hover labels |
| #25 template selection and display | wireframe-only, pink when selected, not pickable in viewports |
| #26 manipulator guides preference | Preferences ▸ Manipulators ▸ Show manipulator guides |
| #28 border edge loop | done (see #27) |
| #29 Maya .ma / .mb | `.ma` open / import / export; `.mb` explains how to resave as `.ma` |
| #31 HumanIK, FBX rigs, skin and weights | FBX import / export with skeletons, skin weights, bind pose and animation; HumanIK skeleton, definitions, control rig, retargeting and baking |
| #30 Hacker News ideas | not started (the issue asks not to implement yet) |
