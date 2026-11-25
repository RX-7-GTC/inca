//! File IO: native `.inca` scenes plus OBJ (tobj), STL (stl_io), glTF/GLB (gltf) and 3MF import,
//! and OBJ/MTL, STL, GLB, 3MF and `.inca` export.

use crate::math::{DMat4, DVec2, DVec3};
use crate::mesh::PolyMesh;
use crate::scene::{Id, Scene, Shape};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

pub const IMPORT_EXTS: &[&str] = &["inca", "ma", "mb", "fbx", "obj", "stl", "3mf", "gltf", "glb", "inca-layer"];
pub const EXPORT_EXTS: &[&str] = &["inca", "ma", "fbx", "obj", "stl", "3mf", "glb"];

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}
fn stem(p: &Path) -> String {
    p.file_stem().and_then(|e| e.to_str()).unwrap_or("import").to_string()
}

pub fn save_scene(s: &mut Scene, path: &Path) -> Result<(), String> {
    s.ensure_anim_layer_nodes();
    s.ensure_display_layer_nodes();
    let data = s.serialize();
    std::fs::write(path, serde_json::to_string(&data).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    s.file = Some(path.to_path_buf());
    s.modified = false;
    Ok(())
}

pub fn open_scene(s: &mut Scene, path: &Path) -> Result<(), String> {
    // a Maya or FBX scene replaces the current one but keeps no .inca path, so Save asks where to write
    if matches!(ext(path).as_str(), "ma" | "mb" | "fbx") {
        if ext(path) == "mb" {
            return Err(crate::maya::MB_MESSAGE.into());
        }
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        s.reset();
        if ext(path) == "fbx" {
            crate::fbx::import_fbx(s, &data, path.parent(), &stem(path))?;
        } else {
            crate::maya::import_ma(s, &String::from_utf8_lossy(&data))?;
        }
        s.modified = false;
        s.undo_stack.clear();
        s.redo_stack.clear();
        return Ok(());
    }
    let txt = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    s.load(&v)?;
    s.file = Some(path.to_path_buf());
    s.modified = false;
    s.undo_stack.clear();
    s.redo_stack.clear();
    Ok(())
}

/// Import a file into the current scene; returns the new top-level nodes.
pub fn import(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    match ext(path).as_str() {
        "inca" => import_inca(s, path),
        "obj" => import_obj(s, path),
        "stl" => import_stl(s, path),
        "3mf" => import_3mf(s, path),
        "gltf" | "glb" => import_gltf(s, path),
        "ma" => crate::maya::import_ma(s, &std::fs::read_to_string(path).map_err(|e| e.to_string())?),
        "mb" => Err(crate::maya::MB_MESSAGE.into()),
        "fbx" => crate::fbx::import_fbx(s, &std::fs::read(path).map_err(|e| e.to_string())?, path.parent(), &stem(path)),
        "inca-layer" => {
            let report = crate::layer_io::import_file(s, path)?;
            if !report.missing_objects.is_empty() || !report.missing_attrs.is_empty() {
                s.warn(format!("Imported layers skipped missing objects/attributes ({} / {})", report.missing_objects.len(), report.missing_attrs.len()));
            }
            Ok(vec![])
        }
        e => Err(format!("can't import .{e} files")),
    }
}

fn import_inca(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    let txt = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    // materials first (remapped to fresh ids)
    let mut mat_map = std::collections::HashMap::new();
    if let Some(mats) = v["shading"]["mats"].as_array() {
        for m in mats {
            let name = m["name"].as_str().unwrap_or("material1");
            if name == "lambert1" || name == "standardSurface1" {
                if let Some(id) = s.material_named(name) {
                    mat_map.insert(m["id"].as_str().unwrap_or("").to_string(), id);
                }
                continue;
            }
            let id = s.create_material(m["type"].as_str().unwrap_or("lambert"), Some(name));
            if let Some(a) = m["attrs"].as_object() {
                for (k, v) in a {
                    s.set_dep_attr(&id, k, v.clone());
                }
            }
            mat_map.insert(m["id"].as_str().unwrap_or("").to_string(), id);
        }
    }
    let mut map: std::collections::HashMap<String, Id> = Default::default();
    let mut made = vec![];
    for d in v["nodes"].as_array().ok_or("not an Inca scene")? {
        if d["cam"]["startup"].as_bool() == Some(true) {
            continue;
        }
        let parent = d["parent"].as_str().and_then(|p| map.get(p).cloned());
        if d["parent"].is_string() && parent.is_none() {
            continue;
        }
        let mut d = d.clone();
        if let Some(o) = d.as_object_mut() {
            o.remove("historyIds");
        }
        if let Some(m) = d["material"].as_str().and_then(|m| mat_map.get(m)) {
            d["material"] = json!(m);
        }
        let id = s.deserialize_node(&d, parent.as_deref(), true);
        map.insert(d["id"].as_str().unwrap_or("").to_string(), id.clone());
        if parent.is_none() {
            made.push(id);
        }
    }
    Ok(made)
}

fn import_obj(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    let opts = tobj::LoadOptions { triangulate: false, single_index: false, ignore_points: true, ignore_lines: true };
    let (models, mats) = tobj::load_obj(path, &opts).map_err(|e| e.to_string())?;
    let mats = mats.unwrap_or_default();
    let mat_ids: Vec<Id> = mats
        .iter()
        .map(|m| {
            let id = s.create_material(if m.shininess.unwrap_or(0.0) > 1.0 { "blinn" } else { "lambert" }, Some(&m.name));
            if let Some(d) = m.diffuse {
                s.set_dep_attr(&id, "color", json!([d[0], d[1], d[2]]));
                s.set_dep_attr(&id, "diffuse", json!(1.0));
            }
            if let Some(tex) = &m.diffuse_texture {
                let t = s.create_texture("file", None);
                let full = path.parent().map(|p| p.join(tex)).unwrap_or_else(|| tex.into());
                s.set_dep_attr(&t, "fileTextureName", json!(full.to_string_lossy()));
                s.set_material_map(&id, "color", &t, "outColor");
            }
            id
        })
        .collect();
    let mut made = vec![];
    for m in models {
        let me = &m.mesh;
        let v: Vec<DVec3> = me.positions.chunks(3).map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)).collect();
        let mut pm = PolyMesh::new(v, vec![]);
        let arities: Vec<u32> = if me.face_arities.is_empty() { vec![3; me.indices.len() / 3] } else { me.face_arities.clone() };
        let has_uv = !me.texcoords.is_empty() && me.texcoord_indices.len() == me.indices.len();
        let mut k = 0usize;
        for a in arities {
            let a = a as usize;
            let f: Vec<u32> = me.indices[k..k + a].to_vec();
            let uv = has_uv.then(|| me.texcoord_indices[k..k + a].iter().map(|&t| DVec2::new(me.texcoords[t as usize * 2] as f64, me.texcoords[t as usize * 2 + 1] as f64)).collect());
            pm.add_face(f, uv, None);
            k += a;
        }
        pm.compact();
        let mat = me.material_id.and_then(|i| mat_ids.get(i)).cloned();
        let name = if m.name.is_empty() || m.name == "unnamed_object" { stem(path) } else { m.name.clone() };
        let id = s.create_mesh_from(pm, &name, mat.as_deref(), None);
        made.push(id);
    }
    Ok(made)
}

fn import_stl(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let m = stl_io::read_stl(&mut f).map_err(|e| e.to_string())?;
    let v: Vec<DVec3> = m.vertices.iter().map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)).collect();
    let faces: Vec<Vec<u32>> = m.faces.iter().map(|t| t.vertices.iter().map(|&i| i as u32).collect()).collect();
    let pm = crate::mesh::op_quadrangulate(&PolyMesh::new(v, faces), 1.0);
    Ok(vec![s.create_mesh_from(pm, &stem(path), None, None)])
}

// ------------------------------------------------------------ 3MF
// A 3MF file is a zip package. Its root model part (found through `_rels/.rels`, normally
// `3D/3dmodel.model`) is XML with mesh objects, component objects that place other objects, and a
// build list. Slicers such as Bambu Studio and PrusaSlicer use the production extension to keep
// objects in further `.model` parts, referenced with a `p:path` attribute.
// Like STL, one Inca unit is one millimetre: imports are converted to millimetres, exports say
// unit="millimeter". 3MF is Z-up (+Z points away from the build plate) and Inca is Y-up, so build
// items are stood up on import and laid back down on export.

/// 3MF/Z-up → Inca/Y-up: (x, y, z) → (x, z, -y).
const Z_UP_TO_Y_UP: DMat4 = DMat4::from_cols_array(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

/// Largest uncompressed `.model` or `.rels` part read from a 3MF package. A 10-million-triangle
/// model is about 700 MB of XML; this stops a corrupt or hostile file from exhausting memory.
const MAX_3MF_PART_BYTES: u64 = 1 << 30;

const NS_3MF_CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const NS_3MF_PROD: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

/// Zip entry name for a package part target ("/3D/3dmodel.model" → "3d/3dmodel.model").
fn part_key_3mf(target: &str) -> String {
    target.trim_start_matches('/').to_ascii_lowercase()
}

/// Millimetres per model unit.
fn unit_mm_3mf(unit: Option<&str>) -> Result<f64, String> {
    Ok(match unit.unwrap_or("millimeter") {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        u => return Err(format!("unknown 3MF unit '{u}'")),
    })
}

/// 3MF transform "m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32" (points are row vectors,
/// so each row is a column here); the translation is scaled from model units to millimetres.
fn matrix_from_3mf(t: Option<&str>, mm: f64) -> Result<DMat4, String> {
    let Some(t) = t else { return Ok(DMat4::IDENTITY) };
    let m: Vec<f64> = t.split_whitespace().map(|x| x.parse::<f64>()).collect::<Result<_, _>>().map_err(|e| format!("bad 3MF transform: {e}"))?;
    if m.len() != 12 {
        return Err(format!("bad 3MF transform '{t}'"));
    }
    Ok(DMat4::from_cols_array(&[m[0], m[1], m[2], 0.0, m[3], m[4], m[5], 0.0, m[6], m[7], m[8], 0.0, m[9] * mm, m[10] * mm, m[11] * mm, 1.0]))
}

fn matrix_to_3mf(m: &DMat4) -> String {
    let c = m.to_cols_array();
    [0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14].iter().map(|&i| c[i].to_string()).collect::<Vec<_>>().join(" ")
}

fn attr_3mf<T: std::str::FromStr>(n: roxmltree::Node, a: &str) -> Result<T, String> {
    n.attribute(a).and_then(|x| x.trim().parse().ok()).ok_or_else(|| format!("3MF <{}> has no valid '{a}'", n.tag_name().name()))
}

fn child_3mf<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == tag)
}

fn import_3mf(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("not a 3MF file: {e}"))?;
    let mut rels = String::new();
    let mut parts: Vec<(String, String)> = vec![];
    for i in 0..zip.len() {
        let e = zip.by_index(i).map_err(|e| e.to_string())?;
        let key = part_key_3mf(e.name());
        if key.ends_with(".model") || key == "_rels/.rels" {
            // read through a limit rather than trusting the size in the zip header
            let mut txt = String::new();
            e.take(MAX_3MF_PART_BYTES + 1).read_to_string(&mut txt).map_err(|err| format!("{key}: {err}"))?;
            if txt.len() as u64 > MAX_3MF_PART_BYTES {
                return Err(format!("3MF part {key} is larger than {} MB", MAX_3MF_PART_BYTES >> 20));
            }
            if key == "_rels/.rels" {
                rels = txt;
            } else {
                parts.push((key, txt));
            }
        }
    }
    let root = roxmltree::Document::parse(&rels)
        .ok()
        .and_then(|d| {
            d.descendants()
                .find(|n| n.tag_name().name() == "Relationship" && n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel")))
                .and_then(|n| n.attribute("Target"))
                .map(part_key_3mf)
        })
        .unwrap_or_else(|| "3d/3dmodel.model".into());
    let docs: Vec<(&str, roxmltree::Document)> =
        parts.iter().map(|(k, t)| roxmltree::Document::parse(t).map(|d| (k.as_str(), d)).map_err(|e| format!("{k}: {e}"))).collect::<Result<_, _>>()?;
    let mut objs = std::collections::HashMap::new();
    let mut units = std::collections::HashMap::new();
    for (k, d) in &docs {
        units.insert(k.to_string(), unit_mm_3mf(d.root_element().attribute("unit"))?);
        for o in d.descendants().filter(|n| n.tag_name().name() == "object") {
            if let Some(id) = o.attribute("id") {
                objs.insert((k.to_string(), id.to_string()), o);
            }
        }
    }
    let doc = docs.iter().find(|(k, _)| *k == root).map(|(_, d)| d).ok_or("3MF file has no 3D model part")?;
    let build = child_3mf(doc.root_element(), "build").ok_or("3MF model has no build section")?;
    let mut made = vec![];
    for item in build.children().filter(|n| n.tag_name().name() == "item") {
        let placed = (|| {
            let part = item.attribute((NS_3MF_PROD, "path")).map(part_key_3mf).unwrap_or_else(|| root.clone());
            let m = Z_UP_TO_Y_UP * matrix_from_3mf(item.attribute("transform"), units[&root])?;
            let oid = item.attribute("objectid").ok_or("3MF build item has no objectid")?;
            place_3mf(s, &objs, &units, &part, oid, m, None, &stem(path), 0)
        })();
        match placed {
            Ok(Some(id)) => made.push(id),
            Ok(None) => {}
            Err(e) => {
                // all or nothing: don't leave the items placed so far in the scene
                for id in made.iter().rev() {
                    s.delete_node(id);
                }
                return Err(e);
            }
        }
    }
    Ok(made)
}

/// Create the node for one 3MF object: a mesh, or a group holding its components.
/// An object used by several components or build items is created once per use, so a slicer
/// file with many copies of a part gets that many independent meshes (not instances).
#[allow(clippy::too_many_arguments)]
fn place_3mf(
    s: &mut Scene,
    objs: &std::collections::HashMap<(String, String), roxmltree::Node>,
    units: &std::collections::HashMap<String, f64>,
    part: &str,
    oid: &str,
    m: DMat4,
    parent: Option<&str>,
    fallback_name: &str,
    depth: usize,
) -> Result<Option<Id>, String> {
    if depth > 32 {
        return Err("3MF components are nested too deeply".into());
    }
    let o = *objs.get(&(part.to_string(), oid.to_string())).ok_or_else(|| format!("3MF refers to a missing object {oid} in {part}"))?;
    let name = o.attribute("name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(fallback_name).to_string();
    let mm = units.get(part).copied().unwrap_or(1.0);
    let id = if let Some(mesh) = child_3mf(o, "mesh") {
        let mut v = vec![];
        let mut faces = vec![];
        for n in mesh.descendants() {
            match n.tag_name().name() {
                "vertex" => v.push(DVec3::new(attr_3mf(n, "x")?, attr_3mf(n, "y")?, attr_3mf(n, "z")?) * mm),
                "triangle" => {
                    let t: [u32; 3] = [attr_3mf(n, "v1")?, attr_3mf(n, "v2")?, attr_3mf(n, "v3")?];
                    if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                        faces.push(t.to_vec());
                    }
                }
                _ => {}
            }
        }
        if faces.iter().flatten().any(|&i| i as usize >= v.len()) {
            return Err(format!("3MF object {oid} has a triangle with a vertex index out of range"));
        }
        if faces.is_empty() {
            return Ok(None);
        }
        // 3MF is triangle data: keep the triangles as they are
        s.create_mesh_from(PolyMesh::new(v, faces), &name, None, parent)
    } else if let Some(comps) = child_3mf(o, "components") {
        let g = s.create_group(&name, parent);
        for c in comps.children().filter(|n| n.tag_name().name() == "component") {
            let placed = (|| {
                let cpart = c.attribute((NS_3MF_PROD, "path")).map(part_key_3mf).unwrap_or_else(|| part.to_string());
                let cm = matrix_from_3mf(c.attribute("transform"), mm)?;
                let cid = c.attribute("objectid").ok_or("3MF component has no objectid")?;
                place_3mf(s, objs, units, &cpart, cid, cm, Some(&g), &name, depth + 1)
            })();
            if let Err(e) = placed {
                // removes the components placed so far with it
                s.delete_node(&g);
                return Err(e);
            }
        }
        g
    } else {
        return Ok(None);
    };
    s.set_local_matrix(&id, m);
    Ok(Some(id))
}

fn xml_escape(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn import_gltf(s: &mut Scene, path: &Path) -> Result<Vec<Id>, String> {
    let (doc, buffers, _images) = gltf::import(path).map_err(|e| e.to_string())?;
    let mats: Vec<Id> = doc
        .materials()
        .map(|m| {
            let id = s.create_material("standardSurface", Some(m.name().unwrap_or("gltfMaterial1")));
            let pbr = m.pbr_metallic_roughness();
            let c = pbr.base_color_factor();
            s.set_dep_attr(&id, "baseColor", json!([c[0], c[1], c[2]]));
            s.set_dep_attr(&id, "base", json!(1.0));
            s.set_dep_attr(&id, "metalness", json!(pbr.metallic_factor()));
            s.set_dep_attr(&id, "specularRoughness", json!(pbr.roughness_factor()));
            let e = m.emissive_factor();
            if e.iter().any(|x| *x > 0.0) {
                s.set_dep_attr(&id, "emission", json!(1.0));
                s.set_dep_attr(&id, "emissionColor", json!(e));
            }
            id
        })
        .collect();
    let mut made = vec![];
    fn walk(s: &mut Scene, n: gltf::Node, parent: Option<&str>, buffers: &[gltf::buffer::Data], mats: &[Id], made: &mut Vec<Id>) {
        let m = DMat4::from_cols_array_2d(&n.transform().matrix().map(|c| c.map(|x| x as f64)));
        let name = n.name().unwrap_or(if n.mesh().is_some() { "mesh1" } else { "transform1" }).to_string();
        let id = if let Some(mesh) = n.mesh() {
            let mut pm = PolyMesh::new(vec![], vec![]);
            let mut mat = None;
            for prim in mesh.primitives() {
                let r = prim.reader(|b| Some(&buffers[b.index()]));
                let Some(pos) = r.read_positions() else { continue };
                let base = pm.v.len() as u32;
                pm.v.extend(pos.map(|p| DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)));
                let uvs: Option<Vec<DVec2>> = r.read_tex_coords(0).map(|t| t.into_f32().map(|u| DVec2::new(u[0] as f64, 1.0 - u[1] as f64)).collect());
                let idx: Vec<u32> = match r.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..(pm.v.len() as u32 - base)).collect(),
                };
                let pmat = prim.material().index().and_then(|i| mats.get(i)).cloned();
                if mat.is_none() {
                    mat = pmat.clone();
                }
                for t in idx.chunks(3) {
                    if t.len() < 3 {
                        continue;
                    }
                    let uv = uvs.as_ref().map(|u| t.iter().map(|&i| u[i as usize]).collect());
                    let fm = if pmat != mat { pmat.clone() } else { None };
                    pm.add_face(t.iter().map(|&i| i + base).collect(), uv, fm);
                }
            }
            let pm = crate::mesh::op_merge_vertices(&pm, None, 1e-7);
            let pm = crate::mesh::op_quadrangulate(&pm, 1.0);
            let id = s.create_mesh_from(pm, &name, mat.as_deref(), parent);
            id
        } else {
            s.create_group(&name, parent)
        };
        s.set_local_matrix(&id, m);
        if parent.is_none() {
            made.push(id.clone());
        }
        for c in n.children() {
            walk(s, c, Some(&id), buffers, mats, made);
        }
    }
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or("glTF has no scene")?;
    for n in scene.nodes() {
        walk(s, n, None, &buffers, &mats, &mut made);
    }
    Ok(made)
}

/// Meshes to export (selection with descendants, or everything).
fn export_meshes(s: &Scene, sel_only: bool) -> Vec<Id> {
    let roots: Vec<Id> = if sel_only { s.sel.clone() } else { s.roots.clone() };
    let mut out = vec![];
    for r in roots {
        for i in s.subtree(&r) {
            if s.nodes[&i].mesh().is_some() && s.is_visible(&i) && !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

pub fn export(s: &Scene, path: &Path, sel_only: bool) -> Result<(), String> {
    match ext(path).as_str() {
        "obj" => export_obj(s, path, sel_only),
        "stl" => export_stl(s, path, sel_only),
        "3mf" => export_3mf(s, path, sel_only),
        "glb" => export_glb(s, path, sel_only),
        "ma" => std::fs::write(path, crate::maya::export_ma(s, sel_only)).map_err(|e| e.to_string()),
        "fbx" => std::fs::write(path, crate::fbx::export_fbx(s, sel_only)).map_err(|e| e.to_string()),
        "inca" => {
            let mut data = s.serialize();
            if sel_only {
                let keep: std::collections::HashSet<String> = s.sel.iter().flat_map(|i| s.subtree(i)).collect();
                let nodes: Vec<Value> = data["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|d| keep.contains(d["id"].as_str().unwrap_or("")) || d["cam"]["startup"].as_bool() == Some(true))
                    .map(|d| {
                        let mut d = d.clone();
                        if !d["parent"].as_str().is_some_and(|p| keep.contains(p)) {
                            d["parent"] = Value::Null;
                        }
                        d
                    })
                    .collect();
                data["nodes"] = json!(nodes);
            }
            std::fs::write(path, data.to_string()).map_err(|e| e.to_string())
        }
        e => Err(format!("can't export .{e} files")),
    }
}

fn export_obj(s: &Scene, path: &Path, sel_only: bool) -> Result<(), String> {
    let mut o = String::from("# Inca OBJ export\n");
    let mtl_name = format!("{}.mtl", stem(path));
    o += &format!("mtllib {mtl_name}\n");
    let (mut vbase, mut tbase) = (1usize, 1usize);
    let mut used_mats = vec![];
    for id in export_meshes(s, sel_only) {
        let n = &s.nodes[&id];
        let ms = n.mesh().unwrap();
        let pm = ms.mesh.transformed(&s.world_matrix(&id));
        o += &format!("o {}\n", n.name);
        for p in &pm.v {
            o += &format!("v {} {} {}\n", p.x, p.y, p.z);
        }
        let mut tcount = 0;
        for fi in 0..pm.f.len() {
            if let Some(u) = pm.face_uv(fi) {
                for t in u {
                    o += &format!("vt {} {}\n", t.x, t.y);
                }
                tcount += u.len();
            }
        }
        let mut cur_mat = String::new();
        let mut tk = tbase;
        for fi in 0..pm.f.len() {
            let mat = pm.face_mat(fi).cloned().unwrap_or(ms.material.clone());
            if mat != cur_mat {
                let name = s.material(&mat).map(|m| m.name.clone()).unwrap_or("default".into());
                o += &format!("usemtl {name}\n");
                if !used_mats.contains(&mat) {
                    used_mats.push(mat.clone());
                }
                cur_mat = mat;
            }
            o += "f";
            let has_uv = pm.face_uv(fi).is_some();
            for &vi in &pm.f[fi] {
                if has_uv {
                    o += &format!(" {}/{}", vi as usize + vbase, tk);
                    tk += 1;
                } else {
                    o += &format!(" {}", vi as usize + vbase);
                }
            }
            o += "\n";
        }
        vbase += pm.v.len();
        tbase += tcount;
    }
    std::fs::write(path, o).map_err(|e| e.to_string())?;
    let mut m = String::new();
    for id in used_mats {
        let Some(mat) = s.material(&id) else { continue };
        let p = mat.pbr();
        m += &format!("newmtl {}\nKd {} {} {}\nKs {s} {s} {s}\nNs {}\nd {}\n\n", mat.name, p.base[0], p.base[1], p.base[2], (1.0 - p.roughness) * 200.0, p.opacity, s = p.specular * 0.5);
    }
    std::fs::write(path.with_file_name(mtl_name), m).map_err(|e| e.to_string())
}

fn triangles_world(s: &Scene, sel_only: bool) -> Vec<[DVec3; 3]> {
    let mut tris = vec![];
    for id in export_meshes(s, sel_only) {
        let pm = s.mesh_world(&id).unwrap();
        for fi in 0..pm.f.len() {
            let f = &pm.f[fi];
            for t in pm.triangulate_face(fi) {
                tris.push([pm.v[f[t[0]] as usize], pm.v[f[t[1]] as usize], pm.v[f[t[2]] as usize]]);
            }
        }
    }
    tris
}

fn export_stl(s: &Scene, path: &Path, sel_only: bool) -> Result<(), String> {
    let tris: Vec<stl_io::Triangle> = triangles_world(s, sel_only)
        .into_iter()
        .map(|t| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let c = |p: DVec3| stl_io::Vector::new([p.x as f32, p.y as f32, p.z as f32]);
            stl_io::Triangle { normal: c(n), vertices: [c(t[0]), c(t[1]), c(t[2])] }
        })
        .collect();
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    stl_io::write_stl(&mut f, tris.iter()).map_err(|e| e.to_string())
}

/// 3MF: one mesh object per Inca mesh in local space, placed by a build item with its world
/// transform laid down to Z-up. A mirroring transform (negative determinant) isn't allowed there,
/// so those meshes are written in world space instead.
fn export_3mf(s: &Scene, path: &Path, sel_only: bool) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut objects = String::new();
    let mut items = String::new();
    let mut count = 0;
    for id in export_meshes(s, sel_only) {
        let n = &s.nodes[&id];
        let mut m = s.world_matrix(&id);
        let world;
        let pm = if m.determinant() < 0.0 {
            world = n.mesh().unwrap().mesh.transformed(&m);
            m = DMat4::IDENTITY;
            &world
        } else {
            &n.mesh().unwrap().mesh
        };
        let mut tris = String::new();
        for fi in 0..pm.f.len() {
            let f = &pm.f[fi];
            for t in pm.triangulate_face(fi) {
                let _ = write!(tris, "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", f[t[0]], f[t[1]], f[t[2]]);
            }
        }
        if tris.is_empty() {
            continue;
        }
        count += 1;
        let _ = write!(objects, "<object id=\"{count}\" type=\"model\" name=\"{}\"><mesh><vertices>", xml_escape(&n.name));
        for p in &pm.v {
            let _ = write!(objects, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p.x, p.y, p.z);
        }
        let _ = writeln!(objects, "</vertices><triangles>{tris}</triangles></mesh></object>");
        let _ = writeln!(items, "<item objectid=\"{count}\" transform=\"{}\"/>", matrix_to_3mf(&(Z_UP_TO_Y_UP.inverse() * m)));
    }
    if count == 0 {
        return Err("nothing to export: no visible meshes".into());
    }
    let model = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{NS_3MF_CORE}\">\n<metadata name=\"Application\">Inca</metadata>\n<resources>\n{objects}</resources>\n<build>\n{items}</build>\n</model>\n"
    );
    let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>\n";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>\n";
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [("[Content_Types].xml", content_types), ("_rels/.rels", rels), ("3D/3dmodel.model", model.as_str())] {
        z.start_file(name, opts).map_err(|e| e.to_string())?;
        z.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// Binary glTF: one mesh per object (flat-shaded corners), world transforms baked into nodes.
fn export_glb(s: &Scene, path: &Path, sel_only: bool) -> Result<(), String> {
    let mut bin: Vec<u8> = vec![];
    let mut views = vec![];
    let mut accessors = vec![];
    let mut meshes = vec![];
    let mut nodes = vec![];
    let mut materials = vec![];
    let mut mat_index: std::collections::HashMap<String, usize> = Default::default();
    let push = |bin: &mut Vec<u8>, data: &[u8]| -> (usize, usize) {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let off = bin.len();
        bin.extend_from_slice(data);
        (off, data.len())
    };
    for id in export_meshes(s, sel_only) {
        let n = &s.nodes[&id];
        let ms = n.mesh().unwrap();
        let pm = &ms.mesh;
        let normals = pm.corner_normals();
        let mut pos: Vec<f32> = vec![];
        let mut nrm: Vec<f32> = vec![];
        let mut uvs: Vec<f32> = vec![];
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for fi in 0..pm.f.len() {
            let f = &pm.f[fi];
            let uv = pm.face_uv(fi);
            for t in pm.triangulate_face(fi) {
                for &c in &t {
                    let p = pm.v[f[c] as usize];
                    let q = [p.x as f32, p.y as f32, p.z as f32];
                    for k in 0..3 {
                        lo[k] = lo[k].min(q[k]);
                        hi[k] = hi[k].max(q[k]);
                    }
                    pos.extend_from_slice(&q);
                    let nn = normals[fi][c];
                    nrm.extend_from_slice(&[nn.x as f32, nn.y as f32, nn.z as f32]);
                    let u = uv.map(|u| u[c]).unwrap_or_default();
                    uvs.extend_from_slice(&[u.x as f32, 1.0 - u.y as f32]);
                }
            }
        }
        if pos.is_empty() {
            continue;
        }
        let count = pos.len() / 3;
        let mut attr = serde_json::Map::new();
        for (name, data, ty) in [("POSITION", &pos, "VEC3"), ("NORMAL", &nrm, "VEC3"), ("TEXCOORD_0", &uvs, "VEC2")] {
            let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
            let (off, len) = push(&mut bin, &bytes);
            views.push(json!({ "buffer": 0, "byteOffset": off, "byteLength": len, "target": 34962 }));
            let mut acc = json!({ "bufferView": views.len() - 1, "componentType": 5126, "count": count, "type": ty });
            if name == "POSITION" {
                acc["min"] = json!(lo);
                acc["max"] = json!(hi);
            }
            accessors.push(acc);
            attr.insert(name.into(), json!(accessors.len() - 1));
        }
        let mi = *mat_index.entry(ms.material.clone()).or_insert_with(|| {
            let m = s.material(&ms.material);
            let p = m.as_ref().map(|m| m.pbr()).unwrap_or_default();
            materials.push(json!({
                "name": m.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
                "pbrMetallicRoughness": { "baseColorFactor": [p.base[0], p.base[1], p.base[2], p.opacity], "metallicFactor": p.metalness, "roughnessFactor": p.roughness },
                "emissiveFactor": [p.emission[0].min(1.0), p.emission[1].min(1.0), p.emission[2].min(1.0)],
            }));
            materials.len() - 1
        });
        meshes.push(json!({ "name": n.shape_name, "primitives": [{ "attributes": attr, "material": mi }] }));
        let wm = s.world_matrix(&id).to_cols_array();
        nodes.push(json!({ "name": n.name, "mesh": meshes.len() - 1, "matrix": wm }));
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let gltf = json!({
        "asset": { "version": "2.0", "generator": "Inca" },
        "scene": 0,
        "scenes": [{ "nodes": (0..nodes.len()).collect::<Vec<_>>() }],
        "nodes": nodes, "meshes": meshes, "materials": materials, "accessors": accessors, "bufferViews": views,
        "buffers": [{ "byteLength": bin.len() }],
    });
    let mut js = gltf.to_string().into_bytes();
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut w = |b: &[u8]| f.write_all(b).map_err(|e| e.to_string());
    w(b"glTF")?;
    w(&2u32.to_le_bytes())?;
    w(&(total as u32).to_le_bytes())?;
    w(&(js.len() as u32).to_le_bytes())?;
    w(b"JSON")?;
    w(&js)?;
    w(&(bin.len() as u32).to_le_bytes())?;
    w(b"BIN\0")?;
    w(&bin)
}

/// Shapes other than meshes are skipped by mesh exporters; this counts them for a warning.
pub fn non_mesh_count(s: &Scene) -> usize {
    s.nodes.values().filter(|n| matches!(n.shape, Shape::Curve(_))).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    #[test]
    fn obj_stl_glb_round_trip() {
        let dir = std::env::temp_dir().join(format!("inca-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Scene::new();
        let c = s.create_mesh("polyCube", &Params::new(), None);
        s.set_attr_f(&c, "tx", 3.0);
        for e in ["obj", "stl", "3mf", "glb", "inca"] {
            let p = dir.join(format!("t.{e}"));
            export(&s, &p, false).unwrap();
            let mut s2 = Scene::new();
            let made = import(&mut s2, &p).unwrap();
            assert_eq!(made.len(), 1, "{e}");
            let b = s2.world_bbox(&made[0]);
            assert!((b.center().x - 3.0).abs() < 1e-4, "{e}: {:?}", b);
            let n = &s2.nodes[&made[0]];
            // 3MF keeps the exported triangles; the other importers rebuild the quads
            assert_eq!(n.mesh().unwrap().mesh.f.len(), if e == "3mf" { 12 } else { 6 }, "{e}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A slicer-style 3MF: the build item places a component object in centimetres, and the
    /// component's mesh lives in a separate part (production extension).
    #[test]
    fn imports_3mf_components_from_other_parts() {
        let dir = std::env::temp_dir().join(format!("inca-3mf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("assembly.3mf");
        let root = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="centimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <resources>
  <object id="2" type="model" name="assembly"><components>
   <component p:path="/3D/Objects/part.model" objectid="1" transform="0 1 0 -1 0 0 0 0 1 0 2 0"/>
  </components></object>
 </resources>
 <build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 1 0 0"/></build>
</model>"#;
        let part = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources><object id="1" type="model" name="part"><mesh>
  <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/></vertices>
  <triangles><triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="0" v2="3" v3="2"/><triangle v1="1" v2="2" v3="3"/></triangles>
 </mesh></object></resources>
</model>"#;
        write_3mf(&p, &[("3D/3dmodel.model", root), ("3D/Objects/part.model", part)]);

        let mut s = Scene::new();
        let made = import(&mut s, &p).unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(s.nodes[&made[0]].name, "assembly");
        let part = s.find("part").unwrap();
        assert_eq!(s.nodes[&part].parent.as_deref(), Some(made[0].as_str()));
        assert_eq!(s.nodes[&part].mesh().unwrap().mesh.f.len(), 4);
        // The part is a 10 mm tetrahedron in its own millimetre part; the root part's transforms
        // are in centimetres. The component turns it +90° about Z (x → y, y → -x: with 3MF's row
        // vectors the first row is the image of x) and moves it 2 cm in y, the build item 1 cm in
        // x. Its bounding box is then x 0..10, y 20..30, z 0..10 mm in the Z-up file, so the centre
        // is (5, 25, 5) mm, which is (5, 5, -25) in Y-up Inca. A transposed rotation would put it
        // at (15, 15, 5); applying the root's centimetres to the part's vertices would make it 100 mm.
        let b = s.world_bbox(&part);
        assert!((b.center() - DVec3::new(5.0, 5.0, -25.0)).abs().max_element() < 1e-6, "{b:?}");
        assert!(((b.max - b.min) - DVec3::splat(10.0)).abs().max_element() < 1e-6, "{b:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Write a 3MF package with the standard root relationship plus the given parts.
    fn write_3mf(path: &Path, parts: &[(&str, &str)]) {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#;
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in [("_rels/.rels", rels)].iter().chain(parts) {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(data.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    /// The root model part of a 3MF package, as text.
    fn read_3mf_model(path: &Path) -> String {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut txt = String::new();
        z.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut txt).unwrap();
        txt
    }

    /// Every world-space vertex of `a` has a match in `b` (and the counts agree).
    fn assert_same_world_points(a: &PolyMesh, b: &PolyMesh, what: &str) {
        assert_eq!(a.v.len(), b.v.len(), "{what}: vertex count");
        for p in &a.v {
            let d = b.v.iter().map(|q| q.distance(*p)).fold(f64::MAX, f64::min);
            assert!(d < 1e-6, "{what}: no imported vertex near {p} (closest {d})");
        }
    }

    /// For a convex mesh: every face normal points away from the mesh centre.
    fn assert_faces_point_outward(pm: &PolyMesh, what: &str) {
        let c = pm.v.iter().copied().sum::<DVec3>() / pm.v.len() as f64;
        for fi in 0..pm.f.len() {
            assert!(pm.face_normal(fi).dot(pm.face_center(fi) - c) > 0.0, "{what}: face {fi} points inward");
        }
    }

    fn export_and_reimport_3mf(s: &Scene, name: &str) -> (Scene, Vec<Id>) {
        let dir = std::env::temp_dir().join(format!("inca-3mf-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{name}.3mf"));
        export(s, &p, false).unwrap();
        let mut s2 = Scene::new();
        let made = import(&mut s2, &p).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        (s2, made)
    }

    /// A mesh under a transformed group exports with the group's transform baked into its
    /// build item, so it comes back at the same place in the world.
    #[test]
    fn exports_3mf_parented_mesh_in_world_space() {
        let mut s = Scene::new();
        let g = s.create_group("grp", None);
        let c = s.create_mesh("polyCube", &Params::new(), None);
        s.reparent(&c, Some(&g), None);
        for (attr, v) in [("tx", 10.0), ("ty", -4.0), ("tz", 2.5), ("rx", 30.0), ("ry", 45.0), ("sx", 2.0), ("sy", 2.0), ("sz", 2.0)] {
            s.set_attr_f(&g, attr, v);
        }
        for (attr, v) in [("tx", 1.0), ("rz", 20.0), ("sy", 3.0)] {
            s.set_attr_f(&c, attr, v);
        }
        let want = s.mesh_world(&c).unwrap();
        let (s2, made) = export_and_reimport_3mf(&s, "parented");
        assert_eq!(made.len(), 1);
        let got = s2.mesh_world(&made[0]).unwrap();
        assert_same_world_points(&want, &got, "parented");
        assert_faces_point_outward(&got, "parented");
    }

    /// A mirrored mesh (negative scale) can't be placed by a 3MF transform, so it is written in
    /// world space; its triangles must still face outward.
    #[test]
    fn exports_3mf_mirrored_mesh_with_outward_faces() {
        let mut s = Scene::new();
        let c = s.create_mesh("polyCube", &Params::new(), None);
        for (attr, v) in [("tx", 5.0), ("ry", 30.0), ("sx", -1.0), ("sz", 2.0)] {
            s.set_attr_f(&c, attr, v);
        }
        assert!(s.world_matrix(&c).determinant() < 0.0);
        let want = s.mesh_world(&c).unwrap();
        assert_faces_point_outward(&want, "mirrored source");
        let (s2, made) = export_and_reimport_3mf(&s, "mirrored");
        assert_eq!(made.len(), 1);
        assert!(s2.world_matrix(&made[0]).determinant() > 0.0, "imported transform should not mirror");
        let got = s2.mesh_world(&made[0]).unwrap();
        assert_same_world_points(&want, &got, "mirrored");
        assert_faces_point_outward(&got, "mirrored");
    }

    /// Files in any 3MF unit import at their size in millimetres, export as millimetres and
    /// keep that size when read back.
    #[test]
    fn units_round_trip_through_3mf() {
        let dir = std::env::temp_dir().join(format!("inca-3mf-units-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a unit cube, moved by 2 units along x by its build item
        let verts: String = (0..8).map(|i| format!(r#"<vertex x="{}" y="{}" z="{}"/>"#, i & 1, (i >> 1) & 1, (i >> 2) & 1)).collect();
        let tris = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        let tris: String = tris.iter().map(|t| format!(r#"<triangle v1="{}" v2="{}" v3="{}"/>"#, t[0], t[1], t[2])).collect();
        for (unit, mm) in [("micron", 0.001), ("millimeter", 1.0), ("centimeter", 10.0), ("inch", 25.4), ("foot", 304.8), ("meter", 1000.0)] {
            let model = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><model unit="{unit}" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model" name="box"><mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh></object></resources><build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 2 0 0"/></build></model>"#
            );
            let p = dir.join(format!("{unit}.3mf"));
            write_3mf(&p, &[("3D/3dmodel.model", &model)]);
            let mut s = Scene::new();
            let made = import(&mut s, &p).unwrap();
            let check = |s: &Scene, id: &str, when: &str| {
                let b = s.world_bbox(id);
                let size = b.max - b.min;
                assert!((size - DVec3::splat(mm)).abs().max_element() < 1e-9 * mm.max(1.0), "{unit} {when}: size {size}");
                assert!((b.min.x - 2.0 * mm).abs() < 1e-9 * mm.max(1.0), "{unit} {when}: min x {}", b.min.x);
            };
            check(&s, &made[0], "import");
            let out = dir.join(format!("{unit}-out.3mf"));
            export(&s, &out, false).unwrap();
            assert!(read_3mf_model(&out).contains(r#"unit="millimeter""#), "{unit}: export unit");
            let mut s2 = Scene::new();
            let made2 = import(&mut s2, &out).unwrap();
            check(&s2, &made2[0], "round trip");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A file that fails part-way (a bad triangle in the second build item, or a component that
    /// contains itself) leaves nothing behind in the scene.
    #[test]
    fn failed_3mf_import_leaves_scene_unchanged() {
        let dir = std::env::temp_dir().join(format!("inca-3mf-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tri = r#"<mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh>"#;
        let bad = r#"<mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="99"/></triangles></mesh>"#;
        let cases = [
            ("bad-index", format!(
                r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model">{tri}</object><object id="2" type="model">{bad}</object><object id="3" type="model"><components><component objectid="1"/><component objectid="2"/></components></object></resources><build><item objectid="1"/><item objectid="3"/></build></model>"#
            )),
            ("cycle", format!(
                r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1" type="model">{tri}</object><object id="2" type="model"><components><component objectid="1"/><component objectid="2"/></components></object></resources><build><item objectid="1"/><item objectid="2"/></build></model>"#
            )),
        ];
        for (name, model) in cases {
            let p = dir.join(format!("{name}.3mf"));
            write_3mf(&p, &[("3D/3dmodel.model", &model)]);
            let mut s = Scene::new();
            let before: Vec<Id> = s.nodes.keys().cloned().collect();
            assert!(import(&mut s, &p).is_err(), "{name}");
            let after: Vec<Id> = s.nodes.keys().cloned().collect();
            assert_eq!(before, after, "{name}: nodes left behind");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_unknown_3mf_units() {
        let dir = std::env::temp_dir().join(format!("inca-3mf-badunit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.3mf");
        write_3mf(&p, &[("3D/3dmodel.model", r#"<model unit="parsec" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources/><build/></model>"#)]);
        let err = import(&mut Scene::new(), &p).unwrap_err();
        assert!(err.contains("parsec"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod compat_tests {
    use super::*;

    /// A scene in the exact layout the JS app's serializeScene() writes.
    const JS_SCENE: &str = r#"{"format":"inca-scene","version":"0.1.12","idCounter":40,
      "nodes":[
        {"id":"d3","name":"persp","kind":"camera","parent":null,"t":[28,21,28],"r":[-27.938,45,0],"s":[1,1,1],"ro":"xyz","p":[0,0,0],"pt":[0,0,0],"visible":true,"layer":null,"template":false,"locks":{},"shapeName":"perspShape","extra":null,
         "cam":{"focalLength":35,"near":0.1,"far":10000,"ortho":false,"orthoWidth":30,"coi":44.82,"horizontalFilmAperture":1.417,"verticalFilmAperture":0.945,"startup":true}},
        {"id":"d10","name":"grp","kind":"group","parent":null,"t":[0,1,0],"r":[0,0,0],"s":[1,1,1],"ro":"xyz","p":[0,0,0],"pt":[0,0,0],"visible":true,"layer":"l1","template":false,"locks":{"tx":true},"shapeName":null,"extra":null},
        {"id":"d11","name":"pCube1","kind":"mesh","parent":"d10","t":[2,0,0],"r":[0,30,0],"s":[1,1,1],"ro":"xyz","p":[0,0,0],"pt":[0,0,0],"visible":true,"layer":null,"template":false,"locks":{},"shapeName":"pCubeShape1","extra":null,
         "material":"m1","smoothLevel":0,"nurbs":false,
         "history":[{"id":"h12","name":"polyCube1","type":"polyCube","params":{"width":1,"height":1,"depth":1,"subdivisionsWidth":1,"subdivisionsHeight":1,"subdivisionsDepth":1}}],
         "mesh":{"v":[[-0.5,-0.5,0.5],[0.5,-0.5,0.5],[-0.5,0.5,0.5],[0.5,0.5,0.5],[-0.5,0.5,-0.5],[0.5,0.5,-0.5],[-0.5,-0.5,-0.5],[0.5,-0.5,-0.5]],
                 "f":[[0,1,3,2],[2,3,5,4],[4,5,7,6],[6,7,1,0],[1,7,5,3],[6,0,2,4]],"uv":null,"fm":null,"softAngle":60,"edgeHard":null,"displayEdges":null}},
        {"id":"d20","name":"pointLight1","kind":"light","parent":null,"t":[0,4,0],"r":[0,0,0],"s":[1,1,1],"ro":"xyz","p":[0,0,0],"pt":[0,0,0],"visible":true,"layer":null,"template":false,"locks":{},"shapeName":"pointLightShape1","extra":null,
         "light":{"type":"pointLight","color":[1,0.5,0.25],"intensity":2,"decayRate":0,"shadows":false,"shadowRadius":2}},
        {"id":"d30","name":"joint1","kind":"joint","parent":null,"t":[0,0,0],"r":[0,0,0],"s":[1,1,1],"ro":"xyz","p":[0,0,0],"pt":[0,0,0],"visible":true,"layer":null,"template":false,"locks":{},"shapeName":null,"extra":null,"rig":{"radius":0.5},"jo":[0,0,90]}
      ],
      "shading":{"mats":[{"id":"m1","name":"lambert1","type":"lambert","attrs":{"color":[0.5,0.5,0.5],"transparency":[0,0,0],"ambientColor":[0,0,0],"incandescence":[0,0,0],"diffuse":0.8,"bump":0},"maps":{}}],"texs":[],"defaultMatId":"m1"},
      "layers":[{"id":"l1","name":"layer1","visible":true,"mode":"normal","color":null}],
      "anim":[{"node":"d11","attr":"translateY","keys":[{"t":1,"v":0,"it":"auto","ot":"auto","is":0,"os":0},{"t":10,"v":5,"it":"auto","ot":"auto","is":0,"os":0}],"pre":"constant","post":"constant"}],
      "conns":[],"time":{"current":1,"start":1,"end":120,"animStart":1,"animEnd":200,"fps":24,"playing":false,"loop":"continuous","autoKey":false,"playEvery":false},
      "render":null,"sel":["d11"],"project":null}"#;

    #[test]
    fn loads_js_scene_and_writes_it_back() {
        let v: Value = serde_json::from_str(JS_SCENE).unwrap();
        let mut s = Scene::new();
        s.load(&v).unwrap();
        let c = s.find("pCube1").unwrap();
        assert_eq!(s.node(&c).unwrap().parent.as_deref(), Some("d10"));
        assert_eq!(s.history_node(&c, 0).unwrap().name, "polyCube1");
        assert_eq!(s.sel, vec!["d11".to_string()]);
        // history is live after loading
        let r = s.resolve("polyCube1").unwrap();
        s.set_attr(&r, "subdivisionsWidth", json!(2));
        assert_eq!(s.node(&c).unwrap().mesh().unwrap().mesh.f.len(), 10);
        // animation, layers, locks, light attributes and rig data survive
        s.set_time(10.0);
        assert!((s.get_attr_f(&c, "ty") - 5.0).abs() < 1e-9);
        let g = s.find("grp").unwrap();
        assert!(s.node(&g).unwrap().is_locked("tx"));
        assert_eq!(s.node(&g).unwrap().layer.as_deref(), Some("l1"));
        let l = s.find("pointLight1").unwrap();
        assert_eq!(s.get_attr_f(&l, "intensity"), 2.0);
        let out = s.serialize();
        let j = out["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "joint1").unwrap();
        assert_eq!(j["kind"], "joint");
        assert_eq!(j["rig"]["radius"], 0.5);
        assert_eq!(serde_json::from_value::<[f64; 3]>(j["jo"].clone()).unwrap(), [0.0, 0.0, 90.0]);
        // the startup camera keeps its id and name
        assert!(out["nodes"].as_array().unwrap().iter().any(|n| n["id"] == "d3" && n["name"] == "persp"));
    }
}
