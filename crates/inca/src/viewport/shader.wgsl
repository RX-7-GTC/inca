// Inca viewport shaders: gradient background, shaded meshes, lines.

struct Light {
    pos: vec4<f32>,    // xyz position, w = type (0 ambient, 1 directional, 2 point, 3 spot, 4 area)
    dir: vec4<f32>,    // xyz direction the light points to
    color: vec4<f32>,  // rgb * intensity
    params: vec4<f32>, // x cos(outer), y cos(inner), z decay rate
};

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    cam_pos: vec4<f32>,  // w = 1 for ortho
    cam_dir: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    mode: vec4<f32>,     // x lighting (0 default, 1 all lights, 2 flat, 3 none), y light count, z xray, w backface culling
    post: vec4<f32>,     // x exposure (stops), y gamma
    render: vec4<f32>,   // x tone mapping (0 none, 1 ACES, 2 AgX, 3 Neutral, 4 Linear), y transparent bg
    sky: vec4<f32>,      // x sky present, y show background, z tan(fov_y/2), w aspect
    sky_col: vec4<f32>,  // sky dome colour (rgb), intensity (a)
    lights: array<Light, 8>,
    shadow_mats: array<mat4x4<f32>, 8>, // first face of each shadowing light
    shadow_info: array<vec4<f32>, 8>,   // x layer (-1 none), y type (1 ortho, 2 spot, 3 point), z radius
};

struct Draw {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    mvp: mat4x4<f32>,    // view_proj * model, filled per viewport
    tint: vec4<f32>,     // rgb, a = mix amount
    params: vec4<f32>,   // x unlit, y alpha, z unused, w specular
    params2: vec4<f32>,  // x roughness, y dot half-size, z cull wires/dots, w textured
    shadow: vec4<f32>,   // x receives shadows, y casts shadows
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> draw: Draw;

// ---------------------------------------------------------------- background
struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) t: f32,
    @location(1) ndc: vec2<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> BgOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: BgOut;
    o.pos = vec4(p[i], 0.0, 1.0);
    o.t = p[i].y * 0.5 + 0.5;
    o.ndc = p[i];
    return o;
}

// The JS `proceduralSky` equirect (linear): zenith/horizon above, ground below. The
// equirect row maps to direction.y, so the colour only depends on that.
fn sky_color(s: f32) -> vec3<f32> {
    let zen = vec3(0.30, 0.48, 0.85);
    let hor = vec3(0.92, 0.95, 1.0);
    let gnd = vec3(0.32, 0.29, 0.26);
    let gh = vec3(0.55, 0.52, 0.48);
    if (s >= 0.0) {
        return mix(hor, zen, pow(s, 0.55));
    }
    return mix(gh, gnd, pow(-s, 0.35));
}

// three.js `equirectUv`: u from atan2(z, x), v from asin(y). The texture's row 0 is the
// nadir, matching the JS DataTexture (`v = 0` at the bottom).
fn equirect_uv(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(d.z, d.x) * (1.0 / 6.28318530718) + 0.5;
    let v = asin(clamp(d.y, -1.0, 1.0)) * (1.0 / 3.14159265359) + 0.5;
    return vec2<f32>(u, v);
}

fn sky_radiance(d: vec3<f32>) -> vec3<f32> {
    if (frame.sky.x > 1.5) {
        // trilinear from the CPU-built mip chain (the JS linear-filters the base level only,
        // but the mips avoid shimmering on large HDRIs)
        return textureSample(sky_tex, sky_samp, equirect_uv(d)).rgb;
    }
    return sky_color(clamp(d.y, -1.0, 1.0));
}

// Normalised cosine-weighted environment average (the PMREM roughness-1 irradiance the JS
// `scene.environment` feeds to diffuse shading).
fn sky_irradiance(n: vec3<f32>) -> vec3<f32> {
    if (frame.sky.x > 1.5) {
        return textureSampleLevel(sky_irr, sky_samp, equirect_uv(n), 0.0).rgb;
    }
    return sky_color(clamp(n.y, -1.0, 1.0));
}

@fragment
fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    // Render Settings > Transparent Background: leave the clear alpha at zero
    if (frame.render.y > 0.5) {
        return vec4(0.0);
    }
    if (frame.sky.x > 0.5 && frame.sky.y > 0.5) {
        var dir = frame.cam_dir.xyz;
        if (frame.cam_pos.w < 0.5) {
            let right = normalize(vec3(frame.view[0][0], frame.view[1][0], frame.view[2][0]));
            let up = normalize(vec3(frame.view[0][1], frame.view[1][1], frame.view[2][1]));
            dir = normalize(dir + right * i.ndc.x * frame.sky.z * frame.sky.w + up * i.ndc.y * frame.sky.z);
        }
        var c = sky_radiance(dir) * frame.sky_col.rgb;
        c = tone_map(max(c * exp2(frame.post.x), vec3(0.0)));
        c = pow(max(c, vec3(0.0)), vec3(1.0 / frame.post.y));
        return vec4(linear_to_srgb(c), 1.0);
    }
    return vec4(mix(frame.bg_bottom.rgb, frame.bg_top.rgb, clamp(i.t, 0.0, 1.0)), 1.0);
}

// ---------------------------------------------------------------- meshes
@group(2) @binding(0) var tex_array: texture_2d_array<f32>;
@group(2) @binding(1) var tex_samp: sampler;
// sky dome equirect (mip 0 = background, mips are unused for now), its diffuse irradiance,
// and a repeat-u/clamp-v linear sampler
@group(2) @binding(2) var sky_tex: texture_2d<f32>;
@group(2) @binding(3) var sky_irr: texture_2d<f32>;
@group(2) @binding(4) var sky_samp: sampler;
@group(3) @binding(0) var shadow_map: texture_depth_2d_array;
@group(3) @binding(1) var shadow_samp: sampler_comparison;

const SHADOW_SIZE: f32 = 512.0;
const SHADOW_NEAR: f32 = 0.5;
const SHADOW_FAR: f32 = 500.0;
const SHADOW_BIAS: f32 = -0.0005;
const SHADOW_NORMAL_BIAS: f32 = 0.02;

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    /// x = texture layer (-1 = none), y = scalar tint
    @location(4) tex: vec2<f32>,
    /// additive emissive colour (JS `emissive` / `incandescence`)
    @location(5) emission: vec3<f32>,
};
struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) tex: vec2<f32>,
    @location(5) emission: vec3<f32>,
};

@vertex
fn vs_mesh(v: MeshIn) -> MeshOut {
    var o: MeshOut;
    let w = draw.model * vec4(v.pos, 1.0);
    o.world = w.xyz;
    o.clip = frame.view_proj * w;
    o.normal = (draw.normal * vec4(v.normal, 0.0)).xyz;
    o.color = v.color;
    o.uv = v.uv;
    o.tex = v.tex;
    o.emission = v.emission;
    return o;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3(0.0031308));
}

// ---------------------------------------------------------------- tone mapping
// The five mappings exposed by Render Settings (three.js `ACESFilmicToneMapping`,
// `AgXToneMapping`, `NeutralToneMapping`, `LinearToneMapping`, `NoToneMapping`).
fn tone_aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3(0.0), vec3(1.0));
}

fn tone_agx(c: vec3<f32>) -> vec3<f32> {
    // inset (columns as in the three.js mat3 constructor)
    var color = vec3<f32>(
        dot(c, vec3<f32>(0.856627153315983, 0.137318972929847, 0.11189821299995)),
        dot(c, vec3<f32>(0.0951212405381588, 0.761241990602591, 0.0767994186031903)),
        dot(c, vec3<f32>(0.0482516061458583, 0.101439036467562, 0.811302368396859)),
    );
    color = max(color, vec3(1e-10));
    color = log2(color);
    color = clamp((color + 12.47393) / (4.026069 + 12.47393), vec3(0.0), vec3(1.0));
    // AgXDefaultContrastApprox
    let x2 = color * color;
    let x4 = x2 * x2;
    color = 15.5 * x4 * x2 - 40.14 * x4 * color + 31.96 * x4 - 6.868 * x2 * color + 0.4298 * x2 + 0.1191 * color - 0.00232;
    // outset
    color = vec3<f32>(
        dot(color, vec3<f32>(1.1271005818144368, -0.11060664309660323, -0.016493938717834573)),
        dot(color, vec3<f32>(-0.1413297634984383, 1.157823702216272, -0.016493938717834257)),
        dot(color, vec3<f32>(-0.14132976349843826, -0.11060664309660294, 1.2519364065950405)),
    );
    return pow(max(color, vec3(0.0)), vec3(2.2));
}

fn tone_neutral(c0: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desat = 0.15;
    let x = min(min(c0.r, c0.g), c0.b);
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    var c = c0 - offset;
    let peak = max(max(c.r, c.g), c.b);
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c = c * new_peak / peak;
    let g = 1.0 - 1.0 / (desat * (peak - new_peak) + 1.0);
    return c * (1.0 - g) + new_peak * g;
}

fn tone_map(c: vec3<f32>) -> vec3<f32> {
    let m = frame.render.x;
    if (m < 0.5) {
        return c;
    } else if (m < 1.5) {
        return tone_aces(c);
    } else if (m < 2.5) {
        return tone_agx(c);
    } else if (m < 3.5) {
        return tone_neutral(c);
    }
    return c;
}

// three.js PCFShadowMap: a small comparison-filtered kernel scaled by `shadow.radius`.
fn shadow_pcf(base: i32, uv: vec2<f32>, depth: f32, radius: f32) -> f32 {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return 1.0;
    }
    // The JS renders 2048² maps; scale its `shadowRadius` (in JS texels) into our 512²
    // map so the world-space PCF footprint stays the same. Never go below half a texel.
    const JS_SHADOW_SIZE: f32 = 2048.0;
    let texel = max(radius * (SHADOW_SIZE / JS_SHADOW_SIZE), 0.5) / SHADOW_SIZE;
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + vec2<f32>(f32(x), f32(y)) * texel, base, depth);
        }
    }
    return sum / 9.0;
}

// 1 = lit, 0 = fully shadowed. `n` is the two-sided normal used for shading.
fn shadow_factor(world: vec3<f32>, n: vec3<f32>, pos: vec3<f32>, info: vec4<f32>, m: mat4x4<f32>) -> f32 {
    if (info.x < 0.0) {
        return 1.0;
    }
    let base = i32(info.x);
    let radius = max(info.z, 1.0);
    let biased = world + n * SHADOW_NORMAL_BIAS;
    if (info.y < 2.5) {
        // directional (orthographic) or spot (perspective)
        let p = m * vec4<f32>(biased, 1.0);
        if (p.w <= 0.0) {
            return 1.0;
        }
        let ndc = p.xyz / p.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        return shadow_pcf(base, uv, ndc.z + SHADOW_BIAS, radius);
    }
    // point light: pick the cube face whose axis is largest and look it up like the
    // six 90-degree depth maps rendered by `prepare_shadow_faces`
    let d = biased - pos;
    let ax = abs(d.x);
    let ay = abs(d.y);
    let az = abs(d.z);
    var face = 0;
    var uv2 = vec2<f32>(0.0);
    if (ax >= ay && ax >= az) {
        if (d.x > 0.0) {
            face = 0;
            uv2 = vec2<f32>(-d.z, -d.y) / ax;
        } else {
            face = 1;
            uv2 = vec2<f32>(d.z, -d.y) / ax;
        }
    } else if (ay >= az) {
        if (d.y > 0.0) {
            face = 2;
            uv2 = vec2<f32>(d.x, d.z) / ay;
        } else {
            face = 3;
            uv2 = vec2<f32>(d.x, -d.z) / ay;
        }
    } else {
        if (d.z > 0.0) {
            face = 4;
            uv2 = vec2<f32>(d.x, -d.y) / az;
        } else {
            face = 5;
            uv2 = vec2<f32>(-d.x, -d.y) / az;
        }
    }
    // A cube face's projection depth uses the view-space z (the dominant axis), not the
    // Euclidean distance; using `length(d)` would self-shadow every off-axis fragment.
    let z = max(ax, max(ay, az));
    if (z <= SHADOW_NEAR || z >= SHADOW_FAR) {
        return 1.0;
    }
    let uv = vec2<f32>(uv2.x * 0.5 + 0.5, 0.5 - uv2.y * 0.5);
    // wgpu's 0..1 perspective depth (glam `perspective_rh`)
    let depth = SHADOW_FAR * (z - SHADOW_NEAR) / (z * (SHADOW_FAR - SHADOW_NEAR));
    return shadow_pcf(base + face, uv, depth + SHADOW_BIAS, radius);
}

fn shade(n_in: vec3<f32>, world: vec3<f32>, base: vec3<f32>, front: bool) -> vec3<f32> {
    var n = normalize(n_in);
    var v = normalize(frame.cam_pos.xyz - world);
    if (frame.cam_pos.w > 0.5) {
        v = -frame.cam_dir.xyz;
    }
    // two-sided: light the side that faces the camera (independent of winding / front_facing)
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let spec_k = draw.params.w;
    let rough = max(draw.params2.x, 0.03);
    let shin = 2.0 / (rough * rough) - 2.0;
    if (frame.mode.x < 0.5) {
        // JS `prepareScene`: the default light sits at camera + (-0.15, 0.2, 0) in camera
        // space and aims down the view axis, so the surface-to-light vector is constant
        let right = normalize(vec3(frame.view[0][0], frame.view[1][0], frame.view[2][0]));
        let up = normalize(vec3(frame.view[0][1], frame.view[1][1], frame.view[2][1]));
        let l = normalize(-0.15 * right + 0.2 * up - frame.cam_dir.xyz);
        let d = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let s = spec_k * pow(max(dot(n, h), 0.0), max(shin, 1.0)) * 0.5;
        return base * d + vec3(s);
    }
    if (frame.mode.x > 2.5) {
        return base * 0.06;
    }
    if (frame.mode.x > 1.5) {
        return base;
    }
    var c = vec3(0.0);
    if (frame.sky.x > 0.5) {
        // sky-dome environment (procedural hemisphere or prefiltered HDRI irradiance)
        c = c + base * sky_irradiance(n) * frame.sky_col.rgb * frame.sky_col.w;
    }
    let count = i32(frame.mode.y);
    for (var i = 0; i < count; i = i + 1) {
        let L = frame.lights[i];
        let ty = L.pos.w;
        if (ty < 0.5) {
            c = c + base * L.color.rgb;
            continue;
        }
        var l: vec3<f32>;
        var att = 1.0;
        if (ty < 1.5) {
            l = -normalize(L.dir.xyz);
        } else {
            let d = L.pos.xyz - world;
            let dist = length(d);
            l = d / max(dist, 1e-5);
            if (L.params.z > 0.5) {
                att = 1.0 / pow(max(dist, 0.01), L.params.z);
            }
            if (ty > 2.5 && ty < 3.5) {
                let cd = dot(-l, normalize(L.dir.xyz));
                att = att * smoothstep(L.params.x, L.params.y, cd);
            }
            if (ty > 3.5) {
                att = att * max(dot(-l, normalize(L.dir.xyz)), 0.0);
            }
        }
        let d = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let s = spec_k * pow(max(dot(n, h), 0.0), max(shin, 1.0)) * 0.5;
        var shadow = 1.0;
        if (draw.shadow.x > 0.5) {
            shadow = shadow_factor(world, n, L.pos.xyz, frame.shadow_info[i], frame.shadow_mats[i]);
        }
        c = c + (base * d + vec3(s) * step(0.0001, d)) * L.color.rgb * att * shadow;
    }
    return c;
}

@fragment
fn fs_mesh(i: MeshOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    // the projection flips screen handedness: our counter-clockwise faces arrive as back-facing
    let front = !ff;
    if (frame.mode.w > 0.5 && !front) {
        discard;
    }
    var base = mix(i.color, draw.tint.rgb, draw.tint.a);
    // Hardware Texturing: the map replaces the material colour, tinted by the JS scalar
    // (`m.color = white * base/diffuse` in materials.js `applyAttrs`)
    if (draw.params2.w > 0.5 && i.tex.x >= 0.0) {
        let t = textureSampleLevel(tex_array, tex_samp, i.uv, i32(i.tex.x), 0.0).rgb;
        base = t * i.tex.y;
    }
    var c = base;
    if (draw.params.x < 0.5) {
        // lit surfaces: material colours are linear and the result is gamma-encoded once, as in the
        // JS (three.js sRGB output); the egui texture path shows these values as they are.
        // Emissive is additive and unlit, like three.js `totalEmissiveRadiance`.
        c = shade(i.normal, i.world, base, front) + i.emission;
        c = tone_map(max(c * exp2(frame.post.x), vec3(0.0)));
        c = pow(max(c, vec3(0.0)), vec3(1.0 / frame.post.y));
        c = linear_to_srgb(c);
    } else {
        c = pow(max(c * exp2(frame.post.x), vec3(0.0)), vec3(1.0 / frame.post.y));
    }
    return vec4(c, draw.params.y);
}

// ---------------------------------------------------------------- component dots

struct PointIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) normal: vec3<f32>,
};
struct PointOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_point(v: PointIn, @builtin(vertex_index) vi: u32) -> PointOut {
    // two triangles, expanded to a fixed-size screen-space square per instance
    var corner = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    var o: PointOut;
    o.color = v.color;
    var clip = draw.mvp * vec4(v.pos, 1.0);
    // dots of faces turned away from the camera are skipped (the JS face-centre `dotMat`)
    if (draw.params2.z > 0.5 && dot(v.normal, v.normal) > 0.0) {
        let world = draw.model * vec4(v.pos, 1.0);
        let d = frame.cam_pos.xyz - world.xyz;
        let to_cam = select(normalize(d), -frame.cam_dir.xyz, frame.cam_pos.w > 0.5);
        if (dot((draw.normal * vec4(v.normal, 0.0)).xyz, to_cam) <= 0.0) {
            clip = vec4(0.0, 0.0, 2.0, 1.0);
        }
    }
    let size = max(frame.post.zw, vec2(1.0));
    let off = corner[vi] * vec2(draw.params2.y * 2.0 / size.x, draw.params2.y * 2.0 / size.y) * clip.w;
    clip = clip + vec4(off, 0.0, 0.0);
    o.clip = clip;
    return o;
}

@fragment
fn fs_point(i: PointOut) -> @location(0) vec4<f32> {
    return i.color;
}
struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) normal_b: vec3<f32>,
};
struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var o: LineOut;
    var clip = draw.mvp * vec4(v.pos, 1.0);
    // wires belonging only to faces turned away from the camera are skipped (the JS `cullable`)
    if (draw.params2.z > 0.5 && dot(v.normal, v.normal) > 0.0) {
        let world = draw.model * vec4(v.pos, 1.0);
        let d = frame.cam_pos.xyz - world.xyz;
        let to_cam = select(normalize(d), -frame.cam_dir.xyz, frame.cam_pos.w > 0.5);
        let na = (draw.normal * vec4(v.normal, 0.0)).xyz;
        let nb = (draw.normal * vec4(v.normal_b, 0.0)).xyz;
        if (dot(na, to_cam) <= 0.0 && dot(nb, to_cam) <= 0.0) {
            clip = vec4(0.0, 0.0, 2.0, 1.0);
        }
    }
    o.clip = clip;
    o.color = mix(v.color, vec4(draw.tint.rgb, v.color.a), draw.tint.a);
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    return i.color;
}
