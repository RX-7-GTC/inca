//! Animation layer files: export/import a layer or a layer hierarchy without its models.

use crate::anim::{AnimCurve, AnimLayer};
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
}
