//! Animation layer files: export/import a layer or a layer hierarchy without its models.

use crate::anim::{Anim, AnimCurve, AnimLayer};
use crate::scene::Scene;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

pub const LAYER_FILE_FORMAT: &str = "inca-animation-layers";
pub const LAYER_FILE_VERSION: u32 = 1;
pub const LAYER_FILE_EXT: &str = "inca-layer";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerFile {
    pub format: String,
    pub version: u32,
    pub layers: Vec<ExportedLayer>,
}

/// One layer with every scene-node reference stored by name instead of by ephemeral scene id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportedLayer {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub weight_curve: AnimCurve,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub lock: bool,
    #[serde(default)]
    pub override_mode: bool,
    #[serde(default = "yes")]
    pub passthrough: bool,
    #[serde(default)]
    pub rotation_by_layer: bool,
    #[serde(default)]
    pub scale_additive: bool,
    #[serde(default)]
    pub members: Vec<String>,
    /// "objectName.attribute" strings.
    #[serde(default)]
    pub attrs: Vec<String>,
    #[serde(default)]
    pub ghost: bool,
    #[serde(default)]
    pub ghost_color: [f32; 3],
    /// Curves whose node field is the object name.
    #[serde(default)]
    pub curves: Vec<AnimCurve>,
}

fn one() -> f64 {
    1.0
}

fn yes() -> bool {
    true
}

fn layer_subtree(layers: &[AnimLayer], root: &str) -> Vec<String> {
    let mut out = vec![];
    fn add(layers: &[AnimLayer], id: &str, out: &mut Vec<String>) {
        if out.iter().any(|x| x == id) {
            return;
        }
        out.push(id.to_string());
        for child in layers.iter().filter(|l| l.parent.as_deref() == Some(id)) {
            add(layers, &child.id, out);
        }
    }
    add(layers, root, &mut out);
    out
}

fn exported_name(s: &Scene, id: &str) -> String {
    s.name_of(id)
}

fn exported_attr(s: &Scene, key: &str) -> String {
    match key.rsplit_once('.') {
        Some((node, attr)) => format!("{}.{}", exported_name(s, node), attr),
        None => key.to_string(),
    }
}

/// Export one layer, or its branch when `branch` is true, as layer-file JSON.
pub fn export_json(s: &Scene, root_id: &str, branch: bool) -> Result<String, String> {
    if !s.anim_layers.iter().any(|l| l.id == root_id) {
        return Err("Animation layer not found".into());
    }
    let ids: HashSet<String> = if branch {
        layer_subtree(&s.anim_layers, root_id).into_iter().collect()
    } else {
        [root_id.to_string()].into_iter().collect()
    };
    let mut layers = vec![];
    for l in s.anim_layers.iter().filter(|l| ids.contains(&l.id)) {
        layers.push(ExportedLayer {
            name: l.name.clone(),
            parent: l.parent.as_ref().filter(|p| ids.contains(*p)).and_then(|p| s.anim_layers.iter().find(|l| &l.id == p).map(|l| l.name.clone())),
            weight: l.weight,
            weight_curve: l.weight_curve.clone(),
            mute: l.mute,
            solo: l.solo,
            lock: l.lock,
            override_mode: l.override_mode,
            passthrough: l.passthrough,
            rotation_by_layer: l.rotation_by_layer,
            scale_additive: l.scale_additive,
            members: l.members.iter().map(|id| exported_name(s, id)).collect(),
            attrs: l.attrs.iter().map(|k| exported_attr(s, k)).collect(),
            ghost: l.ghost,
            ghost_color: l.ghost_color,
            curves: l
                .curves
                .values()
                .filter(|c| !c.keys.is_empty())
                .map(|c| {
                    let mut c = c.clone();
                    c.node = exported_name(s, &c.node);
                    c
                })
                .collect(),
        });
    }
    let file = LayerFile { format: LAYER_FILE_FORMAT.into(), version: LAYER_FILE_VERSION, layers };
    serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
}

pub fn export_file(s: &Scene, root_id: &str, branch: bool, path: &Path) -> Result<usize, String> {
    let json = export_json(s, root_id, branch)?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(if branch {
        layer_subtree(&s.anim_layers, root_id).len()
    } else {
        1
    })
}

#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub layers: Vec<String>,
    pub root: Option<String>,
    pub missing_objects: Vec<String>,
    pub missing_attrs: Vec<String>,
}

fn unique_layer_name(base: &str, used: &mut HashSet<String>) -> String {
    let base = if base.trim().is_empty() { "ImportedLayer" } else { base.trim() };
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    for i in 2.. {
        let name = format!("{base}{i}");
        if used.insert(name.clone()) {
            return name;
        }
    }
    unreachable!()
}

/// Import a layer file into the current scene. Missing objects/attributes are skipped and reported.
pub fn import_json(s: &mut Scene, json: &str) -> Result<ImportReport, String> {
    let file: LayerFile = serde_json::from_str(json).map_err(|e| format!("Not an animation layer file: {e}"))?;
    if file.format != LAYER_FILE_FORMAT {
        return Err("Not an Inca animation layer file".into());
    }
    if file.version > LAYER_FILE_VERSION {
        return Err(format!("Animation layer file version {} is too new", file.version));
    }
    if file.layers.is_empty() {
        return Err("Animation layer file contains no layers".into());
    }
    let by_name: std::collections::HashMap<String, String> = s.nodes.values().map(|n| (n.name.clone(), n.id.clone())).collect();
    let mut used_names: HashSet<String> = s.anim_layers.iter().map(|l| l.name.clone()).collect();
    let mut exported_names = std::collections::HashMap::new();
    let mut missing_objects = std::collections::BTreeSet::new();
    let mut missing_attrs = std::collections::BTreeSet::new();
    let mut imported: Vec<AnimLayer> = vec![];
    for src in &file.layers {
        let id = s.new_id("al");
        let name = unique_layer_name(&src.name, &mut used_names);
        exported_names.insert(src.name.clone(), id.clone());
        let mut curves = indexmap::IndexMap::new();
        let mut attrs: Vec<String> = vec![];
        let mut members: Vec<String> = vec![];
        for src_curve in &src.curves {
            let Some(node) = by_name.get(&src_curve.node).cloned() else {
                missing_objects.insert(src_curve.node.clone());
                continue;
            };
            if !s.node(&node).is_some_and(|n| n.is_keyable(&src_curve.attr)) {
                missing_attrs.insert(format!("{}.{}", src_curve.node, src_curve.attr));
                continue;
            }
            let key = Anim::key(&node, &src_curve.attr);
            let mut curve = src_curve.clone();
            curve.node = node.clone();
            curves.insert(key.clone(), curve);
            if !attrs.contains(&key) {
                attrs.push(key);
            }
            if !members.contains(&node) {
                members.push(node);
            }
        }
        for src_key in &src.attrs {
            let Some((node_name, attr)) = src_key.rsplit_once('.') else {
                continue;
            };
            let Some(node) = by_name.get(node_name).cloned() else {
                missing_objects.insert(node_name.to_string());
                continue;
            };
            if !s.node(&node).is_some_and(|n| n.is_keyable(attr)) {
                missing_attrs.insert(src_key.clone());
                continue;
            }
            let key = Anim::key(&node, attr);
            if !attrs.contains(&key) {
                attrs.push(key);
            }
            if !members.contains(&node) {
                members.push(node);
            }
        }
        for member in &src.members {
            if let Some(node) = by_name.get(member) {
                if !members.contains(node) {
                    members.push(node.clone());
                }
            } else {
                missing_objects.insert(member.clone());
            }
        }
        attrs.sort();
        attrs.dedup();
        members.sort();
        members.dedup();
        imported.push(AnimLayer {
            id,
            name,
            weight: src.weight,
            weight_curve: src.weight_curve.clone(),
            mute: src.mute,
            solo: src.solo,
            lock: src.lock,
            override_mode: src.override_mode,
            passthrough: src.passthrough,
            rotation_by_layer: src.rotation_by_layer,
            scale_additive: src.scale_additive,
            members,
            attrs,
            parent: None,
            ghost: src.ghost,
            ghost_color: src.ghost_color,
            curves,
        });
    }
    for src in &file.layers {
        let Some(new_id) = exported_names.get(&src.name) else { continue };
        let parent = src.parent.as_ref().and_then(|p| exported_names.get(p)).cloned();
        if let Some(l) = imported.iter_mut().find(|l| &l.id == new_id) {
            l.parent = parent;
        }
    }
    let root = imported.first().map(|l| l.id.clone());
    let names: Vec<String> = imported.iter().map(|l| l.name.clone()).collect();
    let mut all = imported;
    all.append(&mut s.anim_layers);
    s.anim_layers = all;
    s.normalize_anim_layer_hierarchy();
    if let Some(root) = &root {
        s.anim_layer_current = Some(root.clone());
    }
    let t = s.time.current;
    s.set_time(t);
    s.touch();
    Ok(ImportReport {
        layers: names,
        root,
        missing_objects: missing_objects.into_iter().collect(),
        missing_attrs: missing_attrs.into_iter().collect(),
    })
}

pub fn import_file(s: &mut Scene, path: &Path) -> Result<ImportReport, String> {
    let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    import_json(s, &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Anim;
    use crate::params::Params;
    use crate::scene::Scene;
    use indexmap::IndexMap;

    fn layer(s: &Scene, id: &str, parent: Option<&str>, node: &str, attr: &str) -> AnimLayer {
        let key = Anim::key(node, attr);
        let mut curve = AnimCurve::new(node, attr);
        curve.set_key(0.0, 1.0, "linear");
        let mut curves = IndexMap::new();
        curves.insert(key.clone(), curve);
        AnimLayer {
            id: id.into(),
            name: format!("Layer {id}"),
            weight: 1.0,
            weight_curve: Default::default(),
            mute: false,
            solo: false,
            lock: false,
            override_mode: false,
            passthrough: true,
            rotation_by_layer: false,
            scale_additive: false,
            members: vec![node.into()],
            attrs: vec![key],
            parent: parent.map(str::to_string),
            ghost: false,
            ghost_color: crate::anim::ghost_red(),
            curves,
        }
    }

    #[test]
    fn single_layer_export_uses_object_names() {
        let mut s = Scene::new();
        let node = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![layer(&s, "al1", None, &node, "translateX")];
        let json = export_json(&s, "al1", false).unwrap();
        let file: LayerFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file.format, LAYER_FILE_FORMAT);
        assert_eq!(file.layers.len(), 1);
        let l = &file.layers[0];
        assert_eq!(l.name, "Layer al1");
        assert_eq!(l.members, vec![s.name_of(&node)]);
        assert_eq!(l.attrs, vec![format!("{}.translateX", s.name_of(&node))]);
        assert_eq!(l.curves[0].node, s.name_of(&node));
        assert_eq!(l.curves[0].attr, "translateX");
    }

    #[test]
    fn branch_export_keeps_the_hierarchy() {
        let mut s = Scene::new();
        let node = s.create_mesh("polyCube", &Params::new(), None);
        s.anim_layers = vec![layer(&s, "al1", None, &node, "translateX"), layer(&s, "al2", Some("al1"), &node, "translateY")];
        let file: LayerFile = serde_json::from_str(&export_json(&s, "al1", true).unwrap()).unwrap();
        assert_eq!(file.layers.len(), 2);
        assert_eq!(file.layers[0].parent, None);
        assert_eq!(file.layers[1].parent.as_deref(), Some("Layer al1"));
        let single: LayerFile = serde_json::from_str(&export_json(&s, "al1", false).unwrap()).unwrap();
        assert_eq!(single.layers.len(), 1, "Export Layer must not include children");
    }

    #[test]
    fn import_round_trips_onto_matching_objects() {
        let mut src = Scene::new();
        let node = src.create_mesh("polyCube", &Params::new(), None);
        src.anim_layers = vec![layer(&src, "al1", None, &node, "translateX")];
        let json = export_json(&src, "al1", false).unwrap();
        let mut dst = Scene::new();
        let new_node = dst.create_mesh("polyCube", &Params::new(), None);
        let report = import_json(&mut dst, &json).unwrap();
        assert_eq!(report.layers.len(), 1);
        assert!(report.missing_objects.is_empty() && report.missing_attrs.is_empty());
        let l = &dst.anim_layers[0];
        let key = Anim::key(&new_node, "translateX");
        assert!(l.attrs.contains(&key));
        assert!(l.curves.contains_key(&key));
        assert_eq!(l.members, vec![new_node.clone()]);
        assert_eq!(dst.anim_layer_current.as_deref(), Some(l.id.as_str()));
    }

    #[test]
    fn import_reports_missing_objects_and_keeps_the_layer() {
        let mut src = Scene::new();
        let node = src.create_mesh("polyCube", &Params::new(), None);
        src.anim_layers = vec![layer(&src, "al1", None, &node, "translateX")];
        let json = export_json(&src, "al1", false).unwrap();
        let mut dst = Scene::new();
        let report = import_json(&mut dst, &json).unwrap();
        assert_eq!(report.layers.len(), 1);
        assert!(!report.missing_objects.is_empty());
        let l = &dst.anim_layers[0];
        assert!(l.attrs.is_empty() && l.curves.is_empty() && l.members.is_empty());
        assert!(dst.anim_layer_current.is_some());
    }

    #[test]
    fn import_branch_restores_the_hierarchy() {
        let mut src = Scene::new();
        let node = src.create_mesh("polyCube", &Params::new(), None);
        src.anim_layers = vec![layer(&src, "al1", None, &node, "translateX"), layer(&src, "al2", Some("al1"), &node, "translateY")];
        let json = export_json(&src, "al1", true).unwrap();
        let mut dst = Scene::new();
        dst.create_mesh("polyCube", &Params::new(), None);
        let report = import_json(&mut dst, &json).unwrap();
        assert_eq!(report.layers.len(), 2);
        let root = dst.anim_layers.iter().find(|l| l.name == "Layer al1").unwrap();
        let child = dst.anim_layers.iter().find(|l| l.name == "Layer al2").unwrap();
        assert_eq!(child.parent.as_deref(), Some(root.id.as_str()));
    }

    #[test]
    fn importing_the_same_layer_twice_gets_unique_names() {
        let mut src = Scene::new();
        let node = src.create_mesh("polyCube", &Params::new(), None);
        src.anim_layers = vec![layer(&src, "al1", None, &node, "translateX")];
        let json = export_json(&src, "al1", false).unwrap();
        let mut dst = Scene::new();
        dst.create_mesh("polyCube", &Params::new(), None);
        import_json(&mut dst, &json).unwrap();
        import_json(&mut dst, &json).unwrap();
        assert_eq!(dst.anim_layers.len(), 2);
        assert_ne!(dst.anim_layers[0].name, dst.anim_layers[1].name);
    }

    #[test]
    fn import_rejects_other_json() {
        let mut s = Scene::new();
        assert!(import_json(&mut s, r#"{"format":"something-else","version":1,"layers":[]}"#).is_err());
    }
}
