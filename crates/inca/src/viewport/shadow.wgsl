// Depth-only pass for the viewport shadow maps. `DrawUniform.model` is at the start of the
// draw uniform; group 1 holds the light view-projection for the face being rendered.

struct DrawModel {
    model: mat4x4<f32>,
};

struct ShadowFrame {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> draw: DrawModel;
@group(1) @binding(0) var<uniform> shadow: ShadowFrame;

struct MeshIn {
    @location(0) pos: vec3<f32>,
};

@vertex
fn vs_shadow(v: MeshIn) -> @builtin(position) vec4<f32> {
    return shadow.view_proj * draw.model * vec4(v.pos, 1.0);
}
