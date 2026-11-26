//! Shading nodes: materials and textures (attribute maps, same layout as the JS app).

use crate::params::Params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub struct MatType {
    pub ty: &'static str,
    pub label: &'static str,
    pub defaults: fn() -> Value,
    /// attributes that accept a texture connection
    pub mappable: &'static [&'static str],
}

pub static MAT_TYPES: &[MatType] = &[
    MatType { ty: "lambert", label: "Lambert", defaults: || json!({ "color": [0.5, 0.5, 0.5], "transparency": [0, 0, 0], "ambientColor": [0, 0, 0], "incandescence": [0, 0, 0], "diffuse": 0.8, "bump": 1 }), mappable: &["color", "transparency", "incandescence", "bump"] },
    MatType { ty: "blinn", label: "Blinn", defaults: || json!({ "color": [0.5, 0.5, 0.5], "transparency": [0, 0, 0], "ambientColor": [0, 0, 0], "incandescence": [0, 0, 0], "diffuse": 0.8, "eccentricity": 0.3, "specularRollOff": 0.7, "specularColor": [0.5, 0.5, 0.5], "reflectivity": 0.5, "bump": 1 }), mappable: &["color", "transparency", "incandescence", "specularColor", "bump"] },
    MatType { ty: "phong", label: "Phong", defaults: || json!({ "color": [0.5, 0.5, 0.5], "transparency": [0, 0, 0], "ambientColor": [0, 0, 0], "incandescence": [0, 0, 0], "diffuse": 0.8, "cosinePower": 20, "specularColor": [0.5, 0.5, 0.5], "reflectivity": 0.5, "bump": 1 }), mappable: &["color", "transparency", "incandescence", "specularColor", "bump"] },
    MatType { ty: "standardSurface", label: "Standard Surface", defaults: || json!({ "base": 0.8, "baseColor": [0.8, 0.8, 0.8], "diffuseRoughness": 0, "metalness": 0, "specular": 1, "specularColor": [1, 1, 1], "specularRoughness": 0.2, "specularIOR": 1.5, "transmission": 0, "transmissionColor": [1, 1, 1], "coat": 0, "coatRoughness": 0.1, "sheen": 0, "sheenColor": [1, 1, 1], "emission": 0, "emissionColor": [1, 1, 1], "opacity": [1, 1, 1], "thinWalled": false, "bump": 1 }), mappable: &["baseColor", "specularRoughness", "metalness", "emissionColor", "opacity", "bump"] },
    MatType { ty: "surfaceShader", label: "Surface Shader", defaults: || json!({ "outColor": [0.5, 0.5, 0.5], "outTransparency": [0, 0, 0] }), mappable: &["outColor"] },
];

pub struct TexType {
    pub ty: &'static str,
    pub label: &'static str,
    pub defaults: fn() -> Value,
}

pub static TEX_TYPES: &[TexType] = &[
    TexType { ty: "file", label: "File", defaults: || json!({ "fileTextureName": "", "colorSpace": "sRGB" }) },
    TexType { ty: "checker", label: "Checker", defaults: || json!({ "color1": [1, 1, 1], "color2": [0, 0, 0], "contrast": 1 }) },
    TexType { ty: "ramp", label: "Ramp", defaults: || json!({ "type": "V Ramp", "colors": [[0, [1, 0, 0]], [0.5, [0, 1, 0]], [1, [0, 0, 1]]] }) },
    TexType { ty: "noise", label: "Noise", defaults: || json!({ "color1": [1, 1, 1], "color2": [0, 0, 0], "frequency": 8, "amplitude": 1 }) },
    TexType { ty: "grid", label: "Grid", defaults: || json!({ "lineColor": [1, 1, 1], "fillerColor": [0, 0, 0], "uWidth": 0.1, "vWidth": 0.1 }) },
    TexType { ty: "cloth", label: "Cloth", defaults: || json!({ "uColor": [0.8, 0.75, 0.7], "vColor": [0.55, 0.5, 0.45], "gapColor": [0.15, 0.12, 0.1], "uWidth": 0.75, "vWidth": 0.75, "uWave": 0, "vWave": 0, "randomness": 0, "widthSpread": 0, "brightSpread": 0 }) },
    TexType { ty: "fractal", label: "Fractal", defaults: || json!({ "amplitude": 1, "ratio": 0.707, "threshold": 0, "levelMax": 9, "frequencyRatio": 2, "bias": 0 }) },
    TexType { ty: "mountain", label: "Mountain", defaults: || json!({ "snowColor": [1, 1, 1], "rockColor": [0.262, 0.102, 0], "amplitude": 1, "snowRoughness": 0.4, "rockRoughness": 0.707, "boundary": 1, "snowAltitude": 0.5, "snowDropoff": 2, "snowSlope": 0.8, "depthMax": 20 }) },
    TexType { ty: "water", label: "Water", defaults: || json!({ "numberOfWaves": 8, "waveTime": 0, "waveVelocity": 1, "waveAmplitude": 0.05, "waveFrequency": 4, "subWaveFrequency": 0.125, "smoothness": 2, "windU": 1, "windV": 0 }) },
    TexType { ty: "bulge", label: "Bulge", defaults: || json!({ "uWidth": 0.1, "vWidth": 0.1 }) },
    TexType { ty: "ocean", label: "Ocean", defaults: || json!({ "colorMode": "Wave Height", "numFrequencies": 3, "waveLengthMin": 0.3, "waveLengthMax": 4, "waveHeight": 0.5, "waveTurbulence": 0, "wavePeaking": 0, "waveDirSpread": 0.2, "windU": 1, "windV": 0, "scale": 10, "time": 0, "observerSpeed": 0, "foamThreshold": 0.51, "foamEmission": 0 }) },
];

pub fn mat_type(ty: &str) -> &'static MatType {
    MAT_TYPES.iter().find(|m| m.ty == ty).unwrap_or(&MAT_TYPES[0])
}
pub fn tex_type(ty: &str) -> Option<&'static TexType> {
    TEX_TYPES.iter().find(|m| m.ty == ty)
}

pub fn is_material_kind(kind: &str) -> bool {
    MAT_TYPES.iter().any(|m| m.ty == kind)
}
pub fn is_texture_kind(kind: &str) -> bool {
    TEX_TYPES.iter().any(|m| m.ty == kind)
}
pub const PLACE_KIND: &str = "place2dTexture";
pub const SHADING_ENGINE_KIND: &str = "shadingEngine";

/// Give a new material node its input attributes and Maya output plugs.
pub fn add_material_specs(n: &mut crate::graph::DepNode, ty: &str) {
    if let Value::Object(m) = (mat_type(ty).defaults)() {
        for (k, v) in m {
            n.add_attr(&k, crate::graph::AttrSpec::any(v));
        }
    }
    if n.spec("outColor").is_none() {
        n.add_attr("outColor", crate::graph::AttrSpec::any(json!([0.0, 0.0, 0.0])).read_only());
    }
    if n.spec("outTransparency").is_none() {
        n.add_attr("outTransparency", crate::graph::AttrSpec::any(json!([0.0, 0.0, 0.0])).read_only());
    }
}

/// Give a new texture node its own attributes, the UV input and its output plugs.
pub fn add_texture_specs(n: &mut crate::graph::DepNode, ty: &str) {
    if let Some(t) = tex_type(ty) {
        if let Value::Object(m) = (t.defaults)() {
            for (k, v) in m {
                n.add_attr(&k, crate::graph::AttrSpec::any(v));
            }
        }
    }
    n.add_attr("uvCoord", crate::graph::AttrSpec::vector([0.0, 0.0, 0.0]).keyable(false));
    n.add_attr("outColor", crate::graph::AttrSpec::any(json!([0.0, 0.0, 0.0])).read_only());
    n.add_attr("outAlpha", crate::graph::AttrSpec::float(1.0).read_only());
}

/// Give a place2dTexture node the shared placement attributes and its outUV output.
pub fn add_place_specs(n: &mut crate::graph::DepNode, placement: &Params) {
    for (k, v) in placement {
        n.add_attr(k, crate::graph::AttrSpec::any(v.clone()));
    }
    n.add_attr("outUV", crate::graph::AttrSpec::vector([0.0, 0.0, 0.0]).read_only());
}

/// Maya's shading group inputs: the surface shader and the objects (or face sets) it shades.
pub fn add_shading_engine_specs(n: &mut crate::graph::DepNode) {
    n.add_attr("surfaceShader", crate::graph::AttrSpec::message());
    n.add_attr("dagSetMembers", crate::graph::AttrSpec::message());
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Material {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub attrs: Params,
    /// attribute -> texture id
    #[serde(default)]
    pub maps: Params,
    /// attribute -> texture output ("outAlpha"); absent means outColor
    #[serde(rename = "mapOuts", default, skip_serializing_if = "Params::is_empty")]
    pub map_outs: Params,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Texture {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub attrs: Params,
    /// Connected place2dTexture utility node id.
    #[serde(default)]
    pub place: Option<String>,
}

/// Maya's place2dTexture utility node: shared placement for one 2D texture.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Place2D {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub attrs: Params,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadingData {
    pub mats: Vec<Material>,
    pub texs: Vec<Texture>,
    #[serde(default)]
    pub places: Vec<Place2D>,
    pub default_mat_id: Option<String>,
}

pub fn defaults_for(ty: &str) -> Params {
    match (mat_type(ty).defaults)() {
        Value::Object(m) => m,
        _ => Params::new(),
    }
}
pub fn tex_defaults_for(ty: &str) -> Params {
    let p = match tex_type(ty).map(|t| (t.defaults)()) {
        Some(Value::Object(m)) => m,
        _ => Params::new(),
    };
    p
}

/// Maya's shared 2D placement attributes (normally on place2dTexture).
pub fn add_placement_defaults(p: &mut Params) {
    for (k, v) in [
        ("coverageU", json!(1.0)),
        ("coverageV", json!(1.0)),
        ("translateFrameU", json!(0.0)),
        ("translateFrameV", json!(0.0)),
        ("rotateFrame", json!(0.0)),
        ("mirrorU", json!(false)),
        ("mirrorV", json!(false)),
        ("wrapU", json!("Repeat")),
        ("wrapV", json!("Repeat")),
        ("stagger", json!(false)),
        ("noiseU", json!(0.0)),
        ("noiseV", json!(0.0)),
    ] {
        p.entry(k.to_string()).or_insert(v);
    }
}

fn rgb(v: Option<&Value>) -> Option<[f32; 3]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32])
}
fn f(v: Option<&Value>) -> Option<f32> {
    v?.as_f64().map(|x| x as f32)
}
/// JS `col(specularColor, 1 / lum(specularColor))`: keep the hue but average to 1.
fn normalise_rgb(c: [f32; 3]) -> [f32; 3] {
    let l = ((c[0] + c[1] + c[2]) / 3.0).max(0.01);
    [c[0] / l, c[1] / l, c[2] / l]
}

/// Simplified physically based view of any material, used by the viewport and the path tracer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pbr {
    pub base: [f32; 3],
    pub roughness: f32,
    pub metalness: f32,
    pub specular: f32,
    /// Specular tint (Blinn/Phong normalise it to an average of 1, like the JS).
    pub spec_color: [f32; 3],
    pub emission: [f32; 3],
    pub opacity: f32,
    pub transmission: f32,
    /// Refraction thickness in local space (JS `thickness`: 0 for thin walled, else 0.5).
    pub thickness: f32,
    pub ior: f32,
    pub coat: f32,
    pub coat_roughness: f32,
    pub sheen: f32,
    pub sheen_color: [f32; 3],
    pub unlit: bool,
}

impl Default for Pbr {
    fn default() -> Self {
        Pbr { base: [0.4, 0.4, 0.4], roughness: 0.6, metalness: 0.0, specular: 0.0, spec_color: [1.0; 3], emission: [0.0; 3], opacity: 1.0, transmission: 0.0, thickness: 0.5, ior: 1.5, coat: 0.0, coat_roughness: 0.1, sheen: 0.0, sheen_color: [1.0; 3], unlit: false }
    }
}

impl Material {
    pub fn new(id: String, name: String, ty: &str) -> Self {
        Material { id, name, ty: ty.into(), attrs: defaults_for(ty), maps: Params::new(), map_outs: Params::new() }
    }

    /// Source output of a texture map: "outAlpha", or "outColor" by default.
    pub fn map_out(&self, slot: &str) -> &str {
        self.map_outs.get(slot).and_then(|v| v.as_str()).unwrap_or("outColor")
    }

    pub fn set_map(&mut self, slot: &str, tex: &str, out: &str) {
        self.maps.insert(slot.to_string(), json!(tex));
        if out == "outAlpha" {
            self.map_outs.insert(slot.to_string(), json!("outAlpha"));
        } else {
            self.map_outs.remove(slot);
        }
    }

    pub fn clear_map(&mut self, slot: &str) {
        self.maps.remove(slot);
        self.map_outs.remove(slot);
    }

    pub fn pbr(&self) -> Pbr {
        let a = &self.attrs;
        let mut p = Pbr::default();
        match self.ty.as_str() {
            "standardSurface" => {
                let b = f(a.get("base")).unwrap_or(0.8);
                let c = rgb(a.get("baseColor")).unwrap_or([0.8; 3]);
                p.base = [c[0] * b, c[1] * b, c[2] * b];
                p.roughness = f(a.get("specularRoughness")).unwrap_or(0.2);
                p.metalness = f(a.get("metalness")).unwrap_or(0.0);
                p.specular = f(a.get("specular")).unwrap_or(1.0);
                p.spec_color = rgb(a.get("specularColor")).unwrap_or([1.0; 3]);
                let e = f(a.get("emission")).unwrap_or(0.0);
                let ec = rgb(a.get("emissionColor")).unwrap_or([1.0; 3]);
                p.emission = [ec[0] * e, ec[1] * e, ec[2] * e];
                p.opacity = rgb(a.get("opacity")).map(|o| (o[0] + o[1] + o[2]) / 3.0).unwrap_or(1.0);
                p.transmission = f(a.get("transmission")).unwrap_or(0.0);
                p.thickness = if a.get("thinWalled").and_then(|v| v.as_bool()).unwrap_or(false) { 0.0 } else { 0.5 };
                p.ior = f(a.get("specularIOR")).unwrap_or(1.5);
                p.coat = f(a.get("coat")).unwrap_or(0.0);
                p.coat_roughness = f(a.get("coatRoughness")).unwrap_or(0.1);
                p.sheen = f(a.get("sheen")).unwrap_or(0.0);
                p.sheen_color = rgb(a.get("sheenColor")).unwrap_or([1.0; 3]);
            }
            "surfaceShader" => {
                p.base = rgb(a.get("outColor")).unwrap_or([0.5; 3]);
                p.emission = p.base;
                p.unlit = true;
                p.opacity = 1.0 - rgb(a.get("outTransparency")).map(|o| (o[0] + o[1] + o[2]) / 3.0).unwrap_or(0.0);
            }
            ty => {
                let c = rgb(a.get("color")).unwrap_or([0.5; 3]);
                let d = f(a.get("diffuse")).unwrap_or(0.8);
                p.base = [c[0] * d, c[1] * d, c[2] * d];
                p.emission = rgb(a.get("incandescence")).unwrap_or([0.0; 3]);
                p.opacity = 1.0 - rgb(a.get("transparency")).map(|o| (o[0] + o[1] + o[2]) / 3.0).unwrap_or(0.0);
                match ty {
                    "blinn" => {
                        // JS applyAttrs: roughness = sqrt(eccentricity), specularIntensity =
                        // min(1, lum(specularColor) * 2 * specularRollOff), colour normalised
                        // to an average of 1.
                        p.roughness = f(a.get("eccentricity")).unwrap_or(0.3).max(0.0).sqrt().clamp(0.05, 1.0);
                        let sc = rgb(a.get("specularColor")).unwrap_or([0.5; 3]);
                        let roll = f(a.get("specularRollOff")).unwrap_or(0.7);
                        p.specular = (((sc[0] + sc[1] + sc[2]) / 3.0) * 2.0 * roll).clamp(0.0, 1.0);
                        p.spec_color = normalise_rgb(sc);
                    }
                    "phong" => {
                        let cp = f(a.get("cosinePower")).unwrap_or(20.0).max(2.0);
                        p.roughness = ((2.0 / (cp + 2.0)).sqrt() * 1.2).clamp(0.04, 1.0);
                        let sc = rgb(a.get("specularColor")).unwrap_or([0.5; 3]);
                        p.specular = (((sc[0] + sc[1] + sc[2]) / 3.0) * 2.0).clamp(0.0, 1.0);
                        p.spec_color = normalise_rgb(sc);
                    }
                    _ => {
                        p.roughness = 1.0;
                        p.specular = 0.0;
                    }
                }
            }
        }
        p
    }

    /// Main colour attribute name (for swatches and the texture "color" slot).
    pub fn color_attr(&self) -> &'static str {
        match self.ty.as_str() {
            "standardSurface" => "baseColor",
            "surfaceShader" => "outColor",
            _ => "color",
        }
    }
}

impl Place2D {
    pub fn new(id: String, name: String, texture_type: &str) -> Self {
        let mut attrs = Params::new();
        add_placement_defaults(&mut attrs);
        // Preserve the old default tiling of the checker/grid/cloth textures.
        let repeat = if matches!(texture_type, "checker" | "grid" | "cloth") { 4.0 } else { 1.0 };
        attrs.insert("repeatU".into(), json!(repeat));
        attrs.insert("repeatV".into(), json!(repeat));
        Place2D { id, name, attrs }
    }
}

/// Placement attributes that older scenes stored directly on the texture node.
pub const PLACEMENT_KEYS: &[&str] = &[
    "coverageU", "coverageV", "translateFrameU", "translateFrameV", "rotateFrame",
    "mirrorU", "mirrorV", "wrapU", "wrapV", "repeatU", "repeatV", "offsetU", "offsetV",
    "rotateUV", "stagger", "noiseU", "noiseV",
];

/// Move legacy placement attrs off a texture node into its place2dTexture attrs.
pub fn take_placement_attrs(tex_attrs: &mut Params) -> Params {
    let mut out = Params::new();
    for k in PLACEMENT_KEYS {
        if let Some(v) = tex_attrs.remove(*k) {
            out.insert((*k).to_string(), v);
        }
    }
    out
}

/// Map output UV to source UV through a place2dTexture node's controls.
pub fn place_uv(a: Option<&Params>, u: f64, v: f64) -> (f64, f64) {
    let val = |k: &str| a.and_then(|a| a.get(k));
    let f = |k: &str, d: f64| val(k).and_then(|x| x.as_f64()).unwrap_or(d);
    let flag = |k: &str| val(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let mode = |k: &str| val(k).and_then(|x| x.as_str()).unwrap_or("Repeat");
    let rotate = |x: f64, y: f64, deg: f64| {
        if deg == 0.0 {
            (x, y)
        } else {
            let (s, c) = (-deg).to_radians().sin_cos();
            (x * c - y * s, x * s + y * c)
        }
    };
    let mut u = u;
    let mut v = v;
    let frame_rot = f("rotateFrame", 0.0);
    if frame_rot != 0.0 {
        let (x, y) = rotate(u - 0.5, v - 0.5, frame_rot);
        u = x + 0.5;
        v = y + 0.5;
    }
    let cu = f("coverageU", 1.0).clamp(0.001, 1.0);
    let cv = f("coverageV", 1.0).clamp(0.001, 1.0);
    u = 0.5 + (u - 0.5) / cu;
    v = 0.5 + (v - 0.5) / cv;
    u += f("translateFrameU", 0.0);
    v += f("translateFrameV", 0.0);
    let uv_rot = f("rotateUV", 0.0);
    if uv_rot != 0.0 {
        let (x, y) = rotate(u - 0.5, v - 0.5, uv_rot);
        u = x + 0.5;
        v = y + 0.5;
    }
    u = u * f("repeatU", 1.0) + f("offsetU", 0.0);
    v = v * f("repeatV", 1.0) + f("offsetV", 0.0);
    if flag("stagger") && (v.floor() as i64).rem_euclid(2) == 1 {
        u += 0.5;
    }
    if flag("mirrorU") {
        let cell = u.floor();
        if (cell as i64).rem_euclid(2) == 1 {
            u = cell + 1.0 - (u - cell);
        }
    }
    if flag("mirrorV") {
        let cell = v.floor();
        if (cell as i64).rem_euclid(2) == 1 {
            v = cell + 1.0 - (v - cell);
        }
    }
    let wrap = |x: f64, mode: &str| match mode {
        "Clamp" => x.clamp(0.0, 1.0),
        "Mirror" => {
            let t = x.rem_euclid(2.0);
            if t > 1.0 { 2.0 - t } else { t }
        }
        _ => x.rem_euclid(1.0),
    };
    let wu = mode("wrapU");
    let wv = mode("wrapV");
    // Repeat keeps the unwrapped cell coordinates so Checker/Cloth keep tiling;
    // Clamp and Mirror genuinely change the lookup domain.
    if wu != "Repeat" {
        u = wrap(u, wu);
    }
    if wv != "Repeat" {
        v = wrap(v, wv);
    }
    let nu = f("noiseU", 0.0);
    let nv = f("noiseV", 0.0);
    if nu > 0.0 || nv > 0.0 {
        let cell = (u.floor() * 7.0 + v.floor() * 13.0) as i64;
        u += (hash2(cell, cell + 17) - 0.5) * nu;
        v += (hash2(cell + 3, cell + 29) - 0.5) * nv;
    }
    (u, v)
}

impl Texture {
    /// Evaluate procedural textures at uv; file textures return None (handled by the caller with the image).
    pub fn sample(&self, place: Option<&Params>, u: f64, v: f64) -> Option<[f32; 3]> {
        let a = &self.attrs;
        let (u, v) = place_uv(place, u, v);
        let mix = |c1: [f32; 3], c2: [f32; 3], t: f32| [c1[0] + (c2[0] - c1[0]) * t, c1[1] + (c2[1] - c1[1]) * t, c1[2] + (c2[2] - c1[2]) * t];
        match self.ty.as_str() {
            "checker" => {
                let c1 = rgb(a.get("color1")).unwrap_or([1.0; 3]);
                let c2 = rgb(a.get("color2")).unwrap_or([0.0; 3]);
                Some(if ((u.floor() + v.floor()) as i64).rem_euclid(2) == 0 { c1 } else { c2 })
            }
            "grid" => {
                let lc = rgb(a.get("lineColor")).unwrap_or([1.0; 3]);
                let fc = rgb(a.get("fillerColor")).unwrap_or([0.0; 3]);
                let uw = a.get("uWidth").and_then(|x| x.as_f64()).unwrap_or(0.1);
                let vw = a.get("vWidth").and_then(|x| x.as_f64()).unwrap_or(0.1);
                Some(if u.fract().abs() < uw || v.fract().abs() < vw { lc } else { fc })
            }
            "noise" => {
                let c1 = rgb(a.get("color1")).unwrap_or([1.0; 3]);
                let c2 = rgb(a.get("color2")).unwrap_or([0.0; 3]);
                let fq = a.get("frequency").and_then(|x| x.as_f64()).unwrap_or(8.0);
                Some(mix(c2, c1, value_noise(u * fq, v * fq) as f32))
            }
            "ramp" => {
                let mut stops: Vec<(f64, [f32; 3])> = a
                    .get("colors")
                    .and_then(|c| c.as_array())
                    .map(|l| l.iter().filter_map(|s| Some((s.get(0)?.as_f64()?, rgb(s.get(1))?))).collect())
                    .unwrap_or_default();
                // The editor keeps stops in their original order so dot identities survive
                // drags across one another; sample them in position order.
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                let horizontal = a.get("type").and_then(|t| t.as_str()) == Some("U Ramp");
                let t = if horizontal { u.fract() } else { v.fract() };
                if stops.is_empty() {
                    return Some([t as f32; 3]);
                }
                let mut prev = stops[0];
                for s in &stops {
                    if t <= s.0 {
                        let k = if s.0 > prev.0 { ((t - prev.0) / (s.0 - prev.0)) as f32 } else { 0.0 };
                        return Some(mix(prev.1, s.1, k));
                    }
                    prev = *s;
                }
                Some(prev.1)
            }
            "fractal" => {
                let amp = a.get("amplitude").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let ratio = a.get("ratio").and_then(|x| x.as_f64()).unwrap_or(0.707);
                let threshold = a.get("threshold").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let level_max = a.get("levelMax").and_then(|x| x.as_f64()).unwrap_or(9.0);
                let freq_ratio = a.get("frequencyRatio").and_then(|x| x.as_f64()).unwrap_or(2.0);
                let bias = a.get("bias").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let n = fractal_noise(u * 4.0, v * 4.0, level_max.clamp(1.0, 12.0) as u32, ratio, freq_ratio);
                let n = ((n - 0.5) * amp + 0.5 + bias + threshold).clamp(0.0, 1.0) as f32;
                Some([n, n, n])
            }
            "mountain" => {
                let snow = rgb(a.get("snowColor")).unwrap_or([1.0; 3]);
                let rock = rgb(a.get("rockColor")).unwrap_or([0.262, 0.102, 0.0]);
                let amp = a.get("amplitude").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let snow_rough = a.get("snowRoughness").and_then(|x| x.as_f64()).unwrap_or(0.4);
                let rock_rough = a.get("rockRoughness").and_then(|x| x.as_f64()).unwrap_or(0.707);
                let boundary = a.get("boundary").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let altitude = a.get("snowAltitude").and_then(|x| x.as_f64()).unwrap_or(0.5);
                let dropoff = a.get("snowDropoff").and_then(|x| x.as_f64()).unwrap_or(2.0);
                let slope_max = a.get("snowSlope").and_then(|x| x.as_f64()).unwrap_or(0.8);
                let depth = a.get("depthMax").and_then(|x| x.as_f64()).unwrap_or(20.0);
                let octaves = depth.clamp(1.0, 10.0) as u32;
                let pattern = |x: f64, y: f64| fractal_noise(x * 3.0, y * 3.0, octaves, 0.707, 2.0);
                // The snow slope only needs the broad shape; lower octaves are much cheaper.
                let slope_pattern = |x: f64, y: f64| fractal_noise(x * 3.0, y * 3.0, octaves.min(4), 0.707, 2.0);
                let h = (pattern(u, v) - 0.5) * amp + 0.5;
                let ragged = (value_noise(u * 40.0, v * 40.0) - 0.5) * boundary * 0.25;
                let h = (h + ragged).clamp(0.0, 1.0);
                let e = 0.01;
                let dx = slope_pattern(u + e, v) - slope_pattern(u - e, v);
                let dy = slope_pattern(u, v + e) - slope_pattern(u, v - e);
                let slope = (dx * dx + dy * dy).sqrt() * 8.0;
                let line = 1.0 - altitude;
                let snow_amt = smoothstep(line, line + 0.25 / dropoff.max(0.1), h) * (1.0 - (slope / slope_max.max(0.1)).clamp(0.0, 1.0));
                let rock_b = 1.0 - rock_rough as f32 * 0.35 * value_noise(u * 50.0, v * 50.0) as f32;
                let snow_b = 1.0 - snow_rough as f32 * 0.25 * value_noise(u * 30.0, v * 30.0) as f32;
                let rock = [rock[0] * rock_b, rock[1] * rock_b, rock[2] * rock_b];
                let snow = [snow[0] * snow_b, snow[1] * snow_b, snow[2] * snow_b];
                Some(mix(rock, snow, snow_amt as f32))
            }
            "cloth" => {
                let u_col = rgb(a.get("uColor")).unwrap_or([1.0; 3]);
                let v_col = rgb(a.get("vColor")).unwrap_or([0.5; 3]);
                let gap = rgb(a.get("gapColor")).unwrap_or([0.0; 3]);
                let u_width = a.get("uWidth").and_then(|x| x.as_f64()).unwrap_or(0.75);
                let v_width = a.get("vWidth").and_then(|x| x.as_f64()).unwrap_or(0.75);
                let u_wave = a.get("uWave").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let v_wave = a.get("vWave").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let random = a.get("randomness").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let width_spread = a.get("widthSpread").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let bright_spread = a.get("brightSpread").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let ju = (value_noise(u * 9.0, v * 9.0) - 0.5) * random * 0.4;
                let jv = (value_noise(u * 11.0 + 7.0, v * 11.0 + 7.0) - 0.5) * random * 0.4;
                let cell_u = (u + ju).floor();
                let cell_v = (v + jv).floor();
                let uu = (u + ju + (v * std::f64::consts::TAU).sin() * v_wave * 0.5).rem_euclid(1.0);
                let vv = (v + jv + (u * std::f64::consts::TAU).sin() * u_wave * 0.5).rem_euclid(1.0);
                let uw = (u_width - width_spread * hash2(cell_v as i64, 13)).clamp(0.0, 1.0);
                let vw = (v_width - width_spread * hash2(17, cell_u as i64)).clamp(0.0, 1.0);
                let band = |x: f64, w: f64| {
                    let edge = 0.04;
                    if x < (w - edge).max(0.0) {
                        1.0
                    } else if x < w {
                        ((w - x) / edge).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                };
                let ub = band(vv, uw) as f32;
                let vb = band(uu, vw) as f32;
                let u_bright = 1.0 - bright_spread as f32 * value_noise(cell_u * 5.0, v * 8.0) as f32;
                let v_bright = 1.0 - bright_spread as f32 * value_noise(u * 8.0, cell_v * 5.0) as f32;
                let ut = mix(gap, [u_col[0] * u_bright, u_col[1] * u_bright, u_col[2] * u_bright], ub);
                let vt = mix(gap, [v_col[0] * v_bright, v_col[1] * v_bright, v_col[2] * v_bright], vb);
                let over_u = (cell_u as i64 + cell_v as i64).rem_euclid(2) == 0;
                Some(if over_u {
                    if ub > 0.0 { ut } else { vt }
                } else if vb > 0.0 {
                    vt
                } else {
                    ut
                })
            }
            "water" => {
                let count = a.get("numberOfWaves").and_then(|x| x.as_f64()).unwrap_or(8.0).clamp(0.0, 12.0) as u32;
                let amplitude = a.get("waveAmplitude").and_then(|x| x.as_f64()).unwrap_or(0.05);
                let frequency = a.get("waveFrequency").and_then(|x| x.as_f64()).unwrap_or(4.0);
                let sub_frequency = a.get("subWaveFrequency").and_then(|x| x.as_f64()).unwrap_or(0.125);
                let smoothness = a.get("smoothness").and_then(|x| x.as_f64()).unwrap_or(2.0);
                let wind_u = a.get("windU").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let wind_v = a.get("windV").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let time = a.get("waveTime").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let velocity = a.get("waveVelocity").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let mut sum = 0.0;
                let mut norm = 0.0;
                for i in 0..count {
                    let f = frequency * (i as f64 + 1.0);
                    let phase = (u * wind_u + v * wind_v) * f * std::f64::consts::TAU + time * velocity * (i as f64 + 1.0);
                    let amp = 1.0 / (i as f64 + 1.0);
                    sum += phase.sin() * amp;
                    norm += amp;
                }
                let waves = if norm > 0.0 { sum / norm } else { 0.0 };
                let sub = ((u + v) * frequency * sub_frequency * std::f64::consts::TAU).sin() * (1.0 / (1.0 + smoothness));
                let value = (0.5 + (waves * 0.5 + sub * 0.5) * (amplitude * 10.0)).clamp(0.0, 1.0) as f32;
                Some([value, value, value])
            }
            "bulge" => {
                let uw = a.get("uWidth").and_then(|x| x.as_f64()).unwrap_or(0.1).clamp(0.0, 0.49);
                let vw = a.get("vWidth").and_then(|x| x.as_f64()).unwrap_or(0.1).clamp(0.0, 0.49);
                let x = u.fract();
                let y = v.fract();
                let inner_u = 1.0 - 2.0 * uw;
                let inner_v = 1.0 - 2.0 * vw;
                let dx = if inner_u > 1e-4 { ((x - uw) / inner_u).clamp(0.0, 1.0) * 2.0 - 1.0 } else { 0.0 };
                let dy = if inner_v > 1e-4 { ((y - vw) / inner_v).clamp(0.0, 1.0) * 2.0 - 1.0 } else { 0.0 };
                let b = (1.0 - dx * dx - dy * dy).max(0.0).sqrt() as f32;
                Some([b, b, b])
            }
            "ocean" => {
                let mode = a.get("colorMode").and_then(|v| v.as_str()).unwrap_or("Wave Height");
                let count = a.get("numFrequencies").and_then(|x| x.as_f64()).unwrap_or(3.0).clamp(1.0, 12.0) as u32;
                let wl_a = a.get("waveLengthMin").and_then(|x| x.as_f64()).unwrap_or(0.3);
                let wl_b = a.get("waveLengthMax").and_then(|x| x.as_f64()).unwrap_or(4.0);
                let (wl_min, wl_max) = (wl_a.min(wl_b).max(0.01), wl_a.max(wl_b).max(0.01));
                let wave_height = a.get("waveHeight").and_then(|x| x.as_f64()).unwrap_or(0.5);
                let turbulence = a.get("waveTurbulence").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let peaking = a.get("wavePeaking").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let spread = a.get("waveDirSpread").and_then(|x| x.as_f64()).unwrap_or(0.2);
                let wind_u = a.get("windU").and_then(|x| x.as_f64()).unwrap_or(1.0);
                let wind_v = a.get("windV").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let scale = a.get("scale").and_then(|x| x.as_f64()).unwrap_or(10.0).max(0.01);
                let time = a.get("time").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let observer = a.get("observerSpeed").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let foam_threshold = a.get("foamThreshold").and_then(|x| x.as_f64()).unwrap_or(0.51).clamp(0.0, 0.99);
                let foam_emission = a.get("foamEmission").and_then(|x| x.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0);
                let wind_angle = wind_v.atan2(wind_u);
                let mut sum = 0.0;
                let mut norm = 0.0;
                for i in 0..count {
                    let t = if count == 1 { 0.5 } else { i as f64 / (count - 1) as f64 };
                    let wavelength = wl_min + (wl_max - wl_min) * t;
                    let angle = wind_angle + (hash2(i as i64, 7) - 0.5) * spread * std::f64::consts::PI;
                    let k = (std::f64::consts::TAU / wavelength * (scale / 10.0)).min(64.0);
                    let speed = (wavelength / wl_max).sqrt();
                    let drift = (1.0 - observer.clamp(0.0, 1.0)).max(0.0);
                    let phase = (u * angle.cos() + v * angle.sin()) * k - time * speed * (i as f64 + 1.0) * drift;
                    let rough = (value_noise(u * 6.0 + i as f64 * 7.0, v * 6.0) - 0.5) * turbulence * std::f64::consts::TAU;
                    let amp = 1.0 / (1.0 + i as f64 * 0.6);
                    sum += (phase + rough).sin() * amp * (1.0 + peaking * 0.5);
                    norm += amp;
                }
                let height = (0.5 + if norm > 0.0 { sum / norm } else { 0.0 } * wave_height).clamp(0.0, 1.0);
                let foam = smoothstep(foam_threshold, (foam_threshold + 0.15).min(1.0), height) * (foam_emission * 4.0).min(1.0);
                let color = match mode {
                    "Foam" => {
                        let f = foam as f32;
                        [f, f, f]
                    }
                    "Wave Height and Foam" => {
                        let deep = [0.02, 0.10, 0.18];
                        let crest = [0.22, 0.48, 0.62];
                        let c = mix(deep, crest, height as f32);
                        mix(c, [1.0; 3], foam as f32)
                    }
                    _ => {
                        let g = height as f32;
                        [g, g, g]
                    }
                };
                Some(color)
            }
            _ => None,
        }
    }
}

/// Size of the viewport texture-array layers (one RGBA image per texture).
pub const VIEW_TEX_SIZE: u32 = 256;

fn srgb_byte(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let s = if x <= 0.0031308 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Bilinear sample of a top-down RGBA image at a bottom-up (v=0 at bottom) texture coordinate.
fn sample_image(img: &image::RgbaImage, u: f64, v: f64) -> [u8; 4] {
    let w = img.width() as f64;
    let h = img.height() as f64;
    let x = u.rem_euclid(1.0) * w - 0.5;
    let y = (1.0 - v.rem_euclid(1.0)) * h - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
    let at = |xx: i64, yy: i64| {
        let xx = xx.clamp(0, img.width() as i64 - 1) as u32;
        let yy = yy.clamp(0, img.height() as i64 - 1) as u32;
        img.get_pixel(xx, yy).0
    };
    let mut out = [0u8; 4];
    for c in 0..4 {
        let a = at(x0 as i64, y0 as i64)[c] as f32;
        let b = at(x0 as i64 + 1, y0 as i64)[c] as f32;
        let d = at(x0 as i64, y0 as i64 + 1)[c] as f32;
        let e = at(x0 as i64 + 1, y0 as i64 + 1)[c] as f32;
        let top = a + (b - a) * fx;
        let bot = d + (e - d) * fx;
        out[c] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// RGBA8 sRGB pixels for the viewport: procedural textures rendered at `VIEW_TEX_SIZE²`
/// (the JS `proceduralCanvas`), file textures sampled through the shared 2D placement.
pub fn viewport_texture_pixels(t: &Texture, place: Option<&Params>, project: Option<&str>) -> Option<Vec<u8>> {
    let size = VIEW_TEX_SIZE;
    if t.ty == "file" {
        let name = t.attrs.get("fileTextureName").and_then(|v| v.as_str()).unwrap_or("");
        let p = std::path::Path::new(name);
        let path = if name.is_empty() {
            None
        } else if p.is_absolute() {
            Some(p.to_path_buf())
        } else {
            Some(project.map(|pr| std::path::Path::new(pr).join(p)).unwrap_or_else(|| p.to_path_buf()))
        };
        let img = path.and_then(|p| image::open(p).ok());
        return match img {
            Some(img) => {
                let img = img.to_rgba8();
                let raw = t.attrs.get("colorSpace").and_then(|v| v.as_str()) == Some("Raw");
                let mut out = vec![255u8; (size * size * 4) as usize];
                for y in 0..size {
                    for x in 0..size {
                        let u = (x as f64 + 0.5) / size as f64;
                        let v = (y as f64 + 0.5) / size as f64;
                        let (su, sv) = place_uv(place, u, v);
                        let px = sample_image(&img, su, sv);
                        let k = ((y * size + x) * 4) as usize;
                        for c in 0..3 {
                            // `colorSpace: Raw` (JS `THREE.NoColorSpace`) bytes are linear;
                            // pre-encode them for the sRGB texture array.
                            out[k + c] = if raw { srgb_byte(px[c] as f32 / 255.0) } else { px[c] };
                        }
                        out[k + 3] = px[3];
                    }
                }
                Some(out)
            }
            // JS `solidCanvas([0.5, 0.5, 0.5])` while a file texture is missing
            None => {
                let mut px = vec![255u8; (size * size * 4) as usize];
                for c in px.chunks_mut(4) {
                    c[0] = 128;
                    c[1] = 128;
                    c[2] = 128;
                }
                Some(px)
            }
        };
    }
    let mut out = vec![255u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let u = (x as f64 + 0.5) / size as f64;
            let v = (y as f64 + 0.5) / size as f64;
            let c = t.sample(place, u, v).unwrap_or([0.5, 0.5, 0.5]);
            let k = ((y * size + x) * 4) as usize;
            out[k] = srgb_byte(c[0]);
            out[k + 1] = srgb_byte(c[1]);
            out[k + 2] = srgb_byte(c[2]);
        }
    }
    Some(out)
}

/// Linear RGBA height pixels for the viewport's 16-bit bump array. A 16-bit height map
/// avoids the visible contour steps an 8-bit colour texture produces in bump shading.
pub fn viewport_bump_pixels(t: &Texture, place: Option<&Params>, project: Option<&str>) -> Option<Vec<f32>> {
    let size = VIEW_TEX_SIZE;
    let mut out = if t.ty == "file" {
        let name = t.attrs.get("fileTextureName").and_then(|v| v.as_str()).unwrap_or("");
        let p = std::path::Path::new(name);
        let path = if name.is_empty() {
            None
        } else if p.is_absolute() {
            Some(p.to_path_buf())
        } else {
            Some(project.map(|pr| std::path::Path::new(pr).join(p)).unwrap_or_else(|| p.to_path_buf()))
        };
        let img = path.and_then(|p| image::open(p).ok())?.to_rgba8();
        let raw = t.attrs.get("colorSpace").and_then(|v| v.as_str()) == Some("Raw");
        let lin = |v: u8| {
            let x = v as f32 / 255.0;
            if raw {
                x
            } else if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        let mut out = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let u = (x as f64 + 0.5) / size as f64;
                let v = (y as f64 + 0.5) / size as f64;
                let (su, sv) = place_uv(place, u, v);
                let px = sample_image(&img, su, sv);
                out.extend_from_slice(&[lin(px[0]), lin(px[1]), lin(px[2]), px[3] as f32 / 255.0]);
            }
        }
        out
    } else {
        let mut out = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                // Match `viewport_texture_pixels` row order exactly, so bump is not mirrored.
                let c = t.sample(place, (x as f64 + 0.5) / size as f64, (y as f64 + 0.5) / size as f64).unwrap_or([0.5; 3]);
                out.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
            }
        }
        out
    };
    normalize_bump_pixels(&mut out, size);
    Some(out)
}

/// Keep bump gradients sane. Displacement-style procedural heights (Ocean/Water) can have
/// slopes many times steeper than a normal bump map; scaling those down avoids black,
/// inverted bump normals while leaving gentle maps untouched.
fn normalize_bump_pixels(out: &mut [f32], size: u32) {
    let n = size as usize;
    if n < 2 || out.len() != n * n * 4 {
        return;
    }
    let channel_scale = |out: &[f32], channel: usize| {
        let at = |x: usize, y: usize| out[(y * n + x) * 4 + channel] as f64;
        let mut sum = 0.0;
        let mut count = 1.0;
        for y in 0..n {
            for x in 0..n {
                let xp = (x + 1) % n;
                let xm = (x + n - 1) % n;
                let yp = (y + 1) % n;
                let ym = (y + n - 1) % n;
                let du = (at(xp, y) - at(xm, y)).abs() * 0.5 * n as f64;
                let dv = (at(x, yp) - at(x, ym)).abs() * 0.5 * n as f64;
                sum += du + dv;
                count += 2.0;
            }
        }
        let avg = sum / count;
        (1.5 / avg.max(1e-6)).clamp(0.02, 1.0) as f32
    };
    let rs = channel_scale(out, 0);
    let as_ = channel_scale(out, 3);
    if (rs - 1.0).abs() < 1e-4 && (as_ - 1.0).abs() < 1e-4 {
        return;
    }
    for px in out.chunks_mut(4) {
        px[0] *= rs;
        px[3] *= as_;
    }
}

fn hash2(x: i64, y: i64) -> f64 {
    let mut h = (x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263)) as u64;
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f64 / 65535.0
}
fn value_noise(x: f64, y: f64) -> f64 {
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    let (xf, yf) = (x - x.floor(), y - y.floor());
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let a = hash2(xi, yi);
    let b = hash2(xi + 1, yi);
    let c = hash2(xi, yi + 1);
    let d = hash2(xi + 1, yi + 1);
    let u = s(xf);
    let v = s(yf);
    (a + (b - a) * u) * (1.0 - v) + (c + (d - c) * u) * v
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0).abs().max(1e-9)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn fractal_noise(x: f64, y: f64, octaves: u32, ratio: f64, frequency_ratio: f64) -> f64 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut f = 1.0;
    for _ in 0..octaves.clamp(1, 12) {
        sum += value_noise(x * f, y * f) * amp;
        norm += amp;
        amp *= ratio.clamp(0.05, 1.0);
        f *= frequency_ratio.clamp(1.0, 4.0);
    }
    sum / norm.max(1e-9)
}

/// Hypershade swatch: an RGBA image of a lit sphere with the material (and the procedural
/// texture on its colour slot, if any) over a dark checker background.
pub fn swatch(m: &Material, color_tex: Option<&Texture>, place: Option<&Params>, size: usize) -> Vec<u8> {
    let p = m.pbr();
    let light = glam::Vec3::new(-0.5, 0.6, 0.65).normalize();
    let fill = glam::Vec3::new(0.7, -0.2, 0.4).normalize();
    let mut out = Vec::with_capacity(size * size * 4);
    let enc = |x: f32| ((if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.max(0.0).powf(1.0 / 2.4) - 0.055 }).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let v = 1.0 - (y as f32 + 0.5) / size as f32 * 2.0;
            let r2 = (u * u + v * v) / 0.81;
            let bg = if ((x * 8 / size) + (y * 8 / size)) % 2 == 0 { 0.05 } else { 0.09 };
            if r2 >= 1.0 {
                let b = enc(bg);
                out.extend_from_slice(&[b, b, b, 255]);
                continue;
            }
            let n = glam::Vec3::new(u / 0.9, v / 0.9, (1.0 - r2).sqrt());
            let mut base = p.base;
            if let Some(t) = color_tex {
                let su = 0.5 + n.x.atan2(n.z) / std::f32::consts::TAU;
                let sv = 0.5 + n.y.asin() / std::f32::consts::PI;
                if let Some(c) = t.sample(place, su as f64, sv as f64) {
                    base = c;
                }
            }
            let diff = n.dot(light).max(0.0) + 0.25 * n.dot(fill).max(0.0) + 0.08;
            let h = (light + glam::Vec3::Z).normalize();
            let shin = 2.0 / (p.roughness.max(0.02) * p.roughness.max(0.02)) - 2.0;
            let spec = if p.unlit { 0.0 } else { p.specular * n.dot(h).max(0.0).powf(shin.max(1.0)) * (shin + 8.0) / 25.0 };
            let kd = if p.unlit { 1.0 } else { diff * (1.0 - p.metalness) };
            let sc = [1.0 - p.metalness + p.metalness * base[0], 1.0 - p.metalness + p.metalness * base[1], 1.0 - p.metalness + p.metalness * base[2]];
            let c: Vec<u8> = (0..3).map(|i| {
                let lit = base[i] * kd + spec * sc[i] * 0.6 + p.emission[i];
                enc(lit * p.opacity + bg * (1.0 - p.opacity))
            }).collect();
            out.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    out
}

/// Flat texture swatch (procedural textures; None for file textures).
pub fn texture_swatch(t: &Texture, place: Option<&Params>, size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let c = t.sample(place, (x as f64 + 0.5) / size as f64, 1.0 - (y as f64 + 0.5) / size as f64)?;
            out.extend(c.iter().map(|v| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8));
            out.push(255);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blinn_and_phong_pbr_match_js_apply_attrs() {
        // JS: roughness sqrt(eccentricity), intensity lum(specularColor)*2*rollOff
        let mut blinn = Material::new("m1".into(), "blinn1".into(), "blinn");
        let p = blinn.pbr();
        assert!((p.roughness - 0.3f32.sqrt()).abs() < 1e-5);
        assert!((p.specular - 0.7).abs() < 1e-5);
        assert_eq!(p.spec_color, [1.0; 3]);
        // a tinted specular colour is normalised to an average of 1
        blinn.attrs.insert("specularColor".into(), serde_json::json!([1.0, 0.5, 0.0]));
        let p = blinn.pbr();
        assert!((p.specular - 0.7).abs() < 1e-5);
        assert_eq!(p.spec_color, [2.0, 1.0, 0.0]);
        // JS: roughness sqrt(2/(cosinePower+2))*1.2, intensity lum(specularColor)*2
        let phong = Material::new("m2".into(), "phong1".into(), "phong");
        let p = phong.pbr();
        assert!((p.roughness - ((2.0f32 / 22.0).sqrt() * 1.2)).abs() < 1e-5);
        assert!((p.specular - 1.0).abs() < 1e-5);
        // Standard Surface keeps the authored specular colour (no normalisation)
        let mut ss = Material::new("m3".into(), "standardSurface1".into(), "standardSurface");
        ss.attrs.insert("specularColor".into(), serde_json::json!([0.5, 0.25, 0.1]));
        assert_eq!(ss.pbr().spec_color, [0.5, 0.25, 0.1]);
        // JS applyAttrs copies coat/sheen straight from the material attributes
        let p = ss.pbr();
        assert_eq!((p.coat, p.coat_roughness, p.sheen), (0.0, 0.1, 0.0));
        assert_eq!(p.sheen_color, [1.0; 3]);
        ss.attrs.insert("coat".into(), serde_json::json!(0.7));
        ss.attrs.insert("coatRoughness".into(), serde_json::json!(0.05));
        ss.attrs.insert("sheen".into(), serde_json::json!(0.4));
        ss.attrs.insert("sheenColor".into(), serde_json::json!([0.2, 0.4, 0.6]));
        let p = ss.pbr();
        assert_eq!((p.coat, p.coat_roughness, p.sheen), (0.7, 0.05, 0.4));
        assert_eq!(p.sheen_color, [0.2, 0.4, 0.6]);
    }

    #[test]
    fn transmission_thickness_matches_js_apply_attrs() {
        let mut m = Material::new("m".into(), "glass1".into(), "standardSurface");
        assert_eq!(m.pbr().thickness, 0.5, "JS applyAttrs: thinWalled false -> 0.5");
        m.attrs.insert("thinWalled".into(), serde_json::json!(true));
        assert_eq!(m.pbr().thickness, 0.0, "JS applyAttrs: thinWalled true -> 0");
        assert_eq!(m.pbr().transmission, 0.0);
        m.attrs.insert("transmission".into(), serde_json::json!(0.8));
        assert!((m.pbr().transmission - 0.8).abs() < 1e-6);
        // other material types keep the default thickness (unused)
        assert_eq!(Material::new("m2".into(), "lambert1".into(), "lambert").pbr().thickness, 0.5);
    }

    #[test]
    fn viewport_texture_pixels_render_procedural_patterns() {
        let t = Texture { id: "t1".into(), name: "checker1".into(), ty: "checker".into(), attrs: tex_defaults_for("checker"), place: None };
        let place = Place2D::new("p1".into(), "place2dTexture1".into(), "checker");
        let px = viewport_texture_pixels(&t, Some(&place.attrs), None).unwrap();
        assert_eq!(px.len(), (VIEW_TEX_SIZE * VIEW_TEX_SIZE * 4) as usize);
        let at = |x: u32, y: u32| {
            let k = ((y * VIEW_TEX_SIZE + x) * 4) as usize;
            [px[k], px[k + 1], px[k + 2]]
        };
        // the default checker has four tiles across and black/white squares
        assert_ne!(at(10, 10), at(75, 10));
        assert_eq!(at(10, 10), at(10, 140));
    }

    #[test]
    fn raw_file_textures_keep_linear_values_in_the_srgb_array() {
        let path = std::env::temp_dir().join(format!("inca_raw_tex_{}.png", std::process::id()));
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([128, 64, 32, 255]));
        img.save(&path).unwrap();
        let mut attrs = tex_defaults_for("file");
        attrs.insert("fileTextureName".into(), serde_json::json!(path.display().to_string()));
        attrs.insert("colorSpace".into(), serde_json::json!("sRGB"));
        let srgb = Texture { id: "t3".into(), name: "file1".into(), ty: "file".into(), attrs: attrs.clone(), place: None };
        let px = viewport_texture_pixels(&srgb, None, None).unwrap();
        assert_eq!(&px[..3], &[128, 64, 32], "sRGB bytes pass through unchanged");
        attrs.insert("colorSpace".into(), serde_json::json!("Raw"));
        let raw = Texture { id: "t4".into(), name: "file1".into(), ty: "file".into(), attrs, place: None };
        let px = viewport_texture_pixels(&raw, None, None).unwrap();
        // the raw byte 128 must sample as linear 128/255 after the GPU's sRGB decode
        assert_eq!(px[0], srgb_byte(128.0 / 255.0));
        assert_eq!(px[1], srgb_byte(64.0 / 255.0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_file_textures_fall_back_to_grey() {
        let mut attrs = Params::new();
        attrs.insert("fileTextureName".into(), serde_json::json!("definitely-missing.png"));
        let t = Texture { id: "t2".into(), name: "file1".into(), ty: "file".into(), attrs, place: None };
        let px = viewport_texture_pixels(&t, None, None).unwrap();
        assert_eq!(&px[..4], &[128, 128, 128, 255]);
    }
}
