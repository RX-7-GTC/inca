use crate::GizmoMode;
use crate::math::{ray_to_plane_origin, segment_to_segment, world_to_screen};
use ecolor::Color32;
use enumset::EnumSet;
use epaint::{Mesh, Vertex};
use std::f64::consts::TAU;
use std::ops::{Add, RangeInclusive};

use crate::shape::ShapeBuilder;
use crate::{GizmoDirection, GizmoDrawData, config::PreparedGizmoConfig, gizmo::Ray};
use glam::{DMat3, DMat4, DQuat, DVec3};

use super::Picked;

const ARROW_FADE: RangeInclusive<f64> = 0.95..=0.99;
const PLANE_FADE: RangeInclusive<f64> = 0.70..=0.86;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum TransformKind {
    Axis,
    Plane,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct PickResult {
    pub subgizmo_point: DVec3,
    pub picked: bool,
    pub t: f64,
}

impl Picked for PickResult {
    fn picked(&self) -> bool {
        self.picked
    }
}

struct ArrowParams {
    start: DVec3,
    end: DVec3,
    direction: DVec3,
    length: f64,
}

fn arrow_modes_overlapping(mode: GizmoMode, other_modes: EnumSet<GizmoMode>) -> bool {
    (mode == GizmoMode::TranslateX && other_modes.contains(GizmoMode::ScaleX))
        || (mode == GizmoMode::TranslateY && other_modes.contains(GizmoMode::ScaleY))
        || (mode == GizmoMode::TranslateZ && other_modes.contains(GizmoMode::ScaleZ))
        || (mode == GizmoMode::ScaleX && other_modes.contains(GizmoMode::TranslateX))
        || (mode == GizmoMode::ScaleY && other_modes.contains(GizmoMode::TranslateY))
        || (mode == GizmoMode::ScaleZ && other_modes.contains(GizmoMode::TranslateZ))
}

fn arrow_params(config: &PreparedGizmoConfig, direction: DVec3, mode: GizmoMode) -> ArrowParams {
    let width = (config.scale_factor * config.visuals.stroke_width) as f64;

    let (start, length) = if mode.is_translate() && arrow_modes_overlapping(mode, config.modes) {
        // Modes contain both translate and scale. Use a bit different translate arrow, so the modes do not overlap.
        let length = (config.scale_factor * config.visuals.gizmo_size) as f64;
        let start = direction * (length + (width * 3.0));

        let length = length * 0.2 + width;

        (start, length)
    } else {
        let start = direction * (width * 0.5 + inner_circle_radius(config));
        let mut length = (config.scale_factor * config.visuals.gizmo_size) as f64 - start.length();

        if config.modes.len() > 1 {
            length -= width * 2.0;
        }

        (start, length)
    };

    ArrowParams {
        start,
        end: start + direction * length,
        direction,
        length,
    }
}

fn arrow_visibility(config: &PreparedGizmoConfig, direction: DVec3) -> f64 {
    let dot = config.eye_to_model_dir.dot(direction).abs();
    (1.0 - (dot - *ARROW_FADE.start()) / (*ARROW_FADE.end() - *ARROW_FADE.start())).min(1.0)
}

pub(crate) fn pick_arrow(
    config: &PreparedGizmoConfig,
    ray: Ray,
    direction: GizmoDirection,
    mode: GizmoMode,
) -> PickResult {
    let ray_length = 1e+14;

    let direction = gizmo_normal(config, direction);

    let mut arrow_params = arrow_params(config, direction, mode);
    arrow_params.start += config.translation;
    arrow_params.end += config.translation;

    let (ray_t, subgizmo_t) = segment_to_segment(
        ray.origin,
        ray.origin + ray.direction * ray_length,
        arrow_params.start,
        arrow_params.end,
    );

    let ray_point = ray.origin + ray.direction * ray_length * ray_t;
    let subgizmo_point =
        arrow_params.start + arrow_params.direction * arrow_params.length * subgizmo_t;
    let dist = (ray_point - subgizmo_point).length();

    let visibility = arrow_visibility(config, direction);

    let picked = visibility > 0.0 && dist <= config.focus_distance as f64;

    PickResult {
        subgizmo_point,
        picked,
        t: ray_t,
    }
}

pub(crate) fn pick_plane(
    config: &PreparedGizmoConfig,
    ray: Ray,
    direction: GizmoDirection,
) -> PickResult {
    let origin = plane_global_origin(config, direction);

    let normal = gizmo_normal(config, direction);

    let (t, dist_from_origin) = ray_to_plane_origin(normal, origin, ray.origin, ray.direction);

    let ray_point = ray.origin + ray.direction * t;

    let visibility = plane_visibility(config, direction);

    let picked = visibility > 0.0 && dist_from_origin <= plane_size(config);

    PickResult {
        subgizmo_point: ray_point,
        picked,
        t,
    }
}

pub(crate) fn pick_circle(
    config: &PreparedGizmoConfig,
    ray: Ray,
    radius: f64,
    filled: bool,
) -> PickResult {
    let origin = config.translation;
    let normal = -config.view_forward();

    let (t, dist_from_gizmo_origin) =
        ray_to_plane_origin(normal, origin, ray.origin, ray.direction);

    let hit_pos = ray.origin + ray.direction * t;

    let picked = if filled {
        dist_from_gizmo_origin <= radius + config.focus_distance as f64
    } else {
        (dist_from_gizmo_origin - radius).abs() <= config.focus_distance as f64
    };

    PickResult {
        subgizmo_point: hit_pos,
        picked,
        t,
    }
}

pub(crate) fn draw_arrow(
    config: &PreparedGizmoConfig,
    focused: bool,
    direction: GizmoDirection,
    mode: GizmoMode,
) -> GizmoDrawData {
    let opacity = arrow_visibility(config, gizmo_normal(config, direction));

    if opacity <= 1e-4 {
        return GizmoDrawData::default();
    }

    let color = gizmo_color(config, focused, direction).gamma_multiply(opacity as _);

    let transform = if config.local_space() {
        DMat4::from_rotation_translation(config.rotation, config.translation)
    } else {
        DMat4::from_translation(config.translation)
    };

    let shape_builder = ShapeBuilder::new(
        config.view_projection * transform,
        config.viewport,
        config.pixels_per_point,
    );

    // Cone faces are opaque; `color` below is dimmed by the inactive alpha and would double-
    // darken them (the same issue the scale cubes had).
    let cone_color = gizmo_color_solid(config, focused, direction);
    let direction = gizmo_local_normal(config, direction);

    let arrow_params = arrow_params(config, direction, mode);

    let tip_stroke_width = 2.4 * config.visuals.stroke_width;
    let tip_length = (tip_stroke_width * config.scale_factor) as f64;

    let tip_start = arrow_params.end - arrow_params.direction * tip_length;

    let mut draw_data = GizmoDrawData::default();
    if mode.is_translate() {
        // Maya's move handles end in a 3D cone rather than a flat triangle.
        const CONE_RADIUS_PX: f32 = 5.0;
        const CONE_HEIGHT_PX: f32 = 14.0;
        let cone_base = arrow_params.end
            - arrow_params.direction * (CONE_HEIGHT_PX * config.scale_factor) as f64;
        draw_data = draw_data.add(
            shape_builder
                .line_segment(
                    arrow_params.start,
                    cone_base,
                    (config.visuals.stroke_width, color),
                )
                .into(),
        );
        draw_data = draw_data.add(draw_gizmo_cone(
            config,
            transform.transform_point3(cone_base),
            transform.transform_vector3(arrow_params.direction).normalize_or_zero(),
            CONE_RADIUS_PX,
            CONE_HEIGHT_PX,
            cone_color,
        ));
        return draw_data;
    }

    draw_data = draw_data.add(
        shape_builder
            .line_segment(
                arrow_params.start,
                tip_start,
                (config.visuals.stroke_width, color),
            )
            .into(),
    );

    if mode.is_scale() {
        draw_data = draw_data.add(
            shape_builder
                .line_segment(
                    arrow_params.start,
                    arrow_params.end,
                    (config.visuals.stroke_width, color),
                )
                .into(),
        );
    }

    draw_data
}

/// Local-space endpoint of an axis arrow. Used to place the scale cube on the gizmo's own tip.
pub(crate) fn arrow_end(
    config: &PreparedGizmoConfig,
    direction: GizmoDirection,
    mode: GizmoMode,
) -> DVec3 {
    let direction = gizmo_local_normal(config, direction);
    arrow_params(config, direction, mode).end
}

/// A small solid cube in gizmo space, drawn with the same projection as the rest of the gizmo.
pub(crate) fn draw_gizmo_cube(
    config: &PreparedGizmoConfig,
    center: DVec3,
    half_px: f32,
    color: Color32,
) -> GizmoDrawData {
    let shape_builder = ShapeBuilder::new(
        config.view_projection,
        config.viewport,
        config.pixels_per_point,
    );
    let half = (config.scale_factor * half_px) as f64;
    let rot = if config.local_space() { config.rotation } else { DQuat::IDENTITY };
    let corners = [
        DVec3::new(-1.0, -1.0, -1.0),
        DVec3::new(1.0, -1.0, -1.0),
        DVec3::new(1.0, 1.0, -1.0),
        DVec3::new(-1.0, 1.0, -1.0),
        DVec3::new(-1.0, -1.0, 1.0),
        DVec3::new(1.0, -1.0, 1.0),
        DVec3::new(1.0, 1.0, 1.0),
        DVec3::new(-1.0, 1.0, 1.0),
    ];
    let world: Vec<DVec3> = corners.iter().map(|c| center + rot * (*c * half)).collect();
    let mut projected = Vec::with_capacity(8);
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    for p in &world {
        let Some(q) = world_to_screen(config.viewport, config.view_projection, *p) else { return GizmoDrawData::default() };
        min = (min.0.min(q.x), min.1.min(q.y));
        max = (max.0.max(q.x), max.1.max(q.y));
        projected.push(q);
    }
    if max.0 - min.0 > 80.0 || max.1 - min.1 > 80.0 {
        return GizmoDrawData::default();
    }
    let light = (config.view_up() + config.view_right() * 0.25 + config.view_forward() * 0.75).normalize();
    let shade = |n: DVec3| {
        let k = (0.82 + 0.18 * n.dot(light).max(0.0)) as f32;
        Color32::from_rgb((color.r() as f32 * k) as u8, (color.g() as f32 * k) as u8, (color.b() as f32 * k) as u8)
    };
    let faces = [
        (DVec3::Z, [4, 5, 6, 7]),
        (DVec3::NEG_Z, [1, 0, 3, 2]),
        (DVec3::X, [1, 2, 6, 5]),
        (DVec3::NEG_X, [0, 4, 7, 3]),
        (DVec3::Y, [3, 7, 6, 2]),
        (DVec3::NEG_Y, [0, 1, 5, 4]),
    ];
    let mut draw_data = GizmoDrawData::default();
    for (normal, idx) in faces {
        let n = rot * normal;
        if n.dot(config.eye_to_model_dir) <= 0.0 {
            continue;
        }
        // A face seen edge-on projects to a zero-area quad, and epaint's feathering on that
        // degenerate input produces a sliver across the whole viewport.
        let area = 0.5
            * idx
                .iter()
                .zip(idx.iter().cycle().skip(1))
                .map(|(&a, &b)| {
                    let (a, b) = (projected[a], projected[b]);
                    a.x * b.y - b.x * a.y
                })
                .sum::<f32>();
        if area.abs() < 2.0 {
            continue;
        }
        let pts = idx.map(|i| world[i]);
        draw_data = draw_data.add(shape_builder.polygon(&pts, shade(n), (0.0, Color32::TRANSPARENT)).into());
    }
    draw_data
}

/// A small 3D cone at the tip of an axis arrow, shaded like the scale cubes.
pub(crate) fn draw_gizmo_cone(
    config: &PreparedGizmoConfig,
    base: DVec3,
    dir: DVec3,
    radius_px: f32,
    height_px: f32,
    color: Color32,
) -> GizmoDrawData {
    const SEGMENTS: usize = 12;
    if dir.length_squared() < 1e-12 {
        return GizmoDrawData::default();
    }
    let radius = (config.scale_factor * radius_px) as f64;
    let height = (config.scale_factor * height_px) as f64;
    let tip = base + dir * height;
    let helper = if dir.dot(DVec3::Y).abs() > 0.9 { DVec3::Z } else { DVec3::Y };
    let u = dir.cross(helper).normalize();
    let v = dir.cross(u).normalize();
    let ring: Vec<DVec3> = (0..SEGMENTS)
        .map(|i| {
            let a = TAU * i as f64 / SEGMENTS as f64;
            base + (u * a.cos() + v * a.sin()) * radius
        })
        .collect();

    let Some(tip_screen) = world_to_screen(config.viewport, config.view_projection, tip) else {
        return GizmoDrawData::default();
    };
    let mut ring_screen = Vec::with_capacity(SEGMENTS);
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    for p in &ring {
        let Some(q) = world_to_screen(config.viewport, config.view_projection, *p) else {
            return GizmoDrawData::default();
        };
        min = (min.0.min(q.x), min.1.min(q.y));
        max = (max.0.max(q.x), max.1.max(q.y));
        ring_screen.push(q);
    }
    min = (min.0.min(tip_screen.x), min.1.min(tip_screen.y));
    max = (max.0.max(tip_screen.x), max.1.max(tip_screen.y));
    if max.0 - min.0 > 60.0 || max.1 - min.1 > 60.0 {
        return GizmoDrawData::default();
    }

    let light = (config.view_up() + config.view_right() * 0.25 + config.view_forward() * 0.75).normalize();
    let shade = |n: DVec3| {
        let k = (0.82 + 0.18 * n.dot(light).max(0.0)) as f32;
        Color32::from_rgb((color.r() as f32 * k) as u8, (color.g() as f32 * k) as u8, (color.b() as f32 * k) as u8)
    };

    // A raw triangle mesh, not `Shape::convex_polygon`: epaint's feathering turns the sharp,
    // near-edge-on side triangles into long miter spikes.
    let mut mesh = Mesh::default();
    for i in 0..SEGMENTS {
        let j = (i + 1) % SEGMENTS;
        let n = (ring[j] - ring[i]).cross(tip - ring[i]).normalize_or_zero();
        if n.dot(config.eye_to_model_dir) <= 0.0 {
            continue;
        }
        let face_color = shade(n);
        let base = mesh.vertices.len() as u32;
        for p in [ring_screen[i], ring_screen[j], tip_screen] {
            mesh.vertices.push(Vertex { pos: p, uv: epaint::Pos2::ZERO, color: face_color });
        }
        mesh.indices.extend([base, base + 1, base + 2]);
    }
    GizmoDrawData::from(mesh)
}

pub(crate) fn draw_plane(
    config: &PreparedGizmoConfig,
    focused: bool,
    direction: GizmoDirection,
) -> GizmoDrawData {
    let opacity = plane_visibility(config, direction);

    if opacity <= 1e-4 {
        return GizmoDrawData::default();
    }

    let color = gizmo_color(config, focused, direction).gamma_multiply(opacity as _);

    let transform = if config.local_space() {
        DMat4::from_rotation_translation(config.rotation, config.translation)
    } else {
        DMat4::from_translation(config.translation)
    };

    let shape_builder = ShapeBuilder::new(
        config.view_projection * transform,
        config.viewport,
        config.pixels_per_point,
    );

    let scale = plane_size(config) * 0.5;
    let a = plane_bitangent(direction) * scale;
    let b = plane_tangent(direction) * scale;
    let origin = plane_local_origin(config, direction);
    let corners = [origin - b - a, origin + b - a, origin + b + a, origin - b + a];
    // Near edge-on/inside-the-plane views can project a plane handle across the whole screen.
    let mvp = config.view_projection * transform;
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    for p in corners {
        let Some(q) = world_to_screen(config.viewport, mvp, p) else { return GizmoDrawData::default() };
        min = (min.0.min(q.x), min.1.min(q.y));
        max = (max.0.max(q.x), max.1.max(q.y));
    }
    if max.0 - min.0 > 120.0 || max.1 - min.1 > 120.0 {
        return GizmoDrawData::default();
    }
    // A near edge-on plane can collapse into a long, dashed sliver.
    let area = (corners.iter().zip(corners.iter().cycle().skip(1)).map(|(a, b)| {
        let (a, b) = (
            world_to_screen(config.viewport, mvp, *a).unwrap_or_default(),
            world_to_screen(config.viewport, mvp, *b).unwrap_or_default(),
        );
        a.x * b.y - b.x * a.y
    }).sum::<f32>() * 0.5).abs();
    if area < 40.0 {
        return GizmoDrawData::default();
    }

    let mut draw_data = GizmoDrawData::default();
    draw_data = draw_data.add(
        shape_builder
            .polygon(
                &corners,
                color,
                (0.0, Color32::TRANSPARENT),
            )
            .into(),
    );
    draw_data
}

pub(crate) fn draw_circle(
    config: &PreparedGizmoConfig,
    color: Color32,
    radius: f64,
    filled: bool,
) -> GizmoDrawData {
    if color.a() == 0 {
        return GizmoDrawData::default();
    }

    let rotation = {
        let forward = config.view_forward();
        let right = config.view_right();
        let up = config.view_up();

        DQuat::from_mat3(&DMat3::from_cols(up, -forward, -right))
    };

    let transform = DMat4::from_rotation_translation(rotation, config.translation);

    let shape_builder = ShapeBuilder::new(
        config.view_projection * transform,
        config.viewport,
        config.pixels_per_point,
    );

    let mut draw_data = GizmoDrawData::default();
    if filled {
        draw_data = draw_data.add(
            shape_builder
                .filled_circle(radius, color, (0.0, Color32::TRANSPARENT))
                .into(),
        );
    } else {
        draw_data = draw_data.add(
            shape_builder
                .circle(radius, (config.visuals.stroke_width, color))
                .into(),
        );
    }
    draw_data
}

pub(crate) const fn plane_bitangent(direction: GizmoDirection) -> DVec3 {
    match direction {
        GizmoDirection::X => DVec3::Y,
        GizmoDirection::Y => DVec3::Z,
        GizmoDirection::Z => DVec3::X,
        GizmoDirection::View => DVec3::ZERO, // Unused
    }
}

pub(crate) const fn plane_tangent(direction: GizmoDirection) -> DVec3 {
    match direction {
        GizmoDirection::X => DVec3::Z,
        GizmoDirection::Y => DVec3::X,
        GizmoDirection::Z => DVec3::Y,
        GizmoDirection::View => DVec3::ZERO, // Unused
    }
}

pub(crate) fn plane_size(config: &PreparedGizmoConfig) -> f64 {
    (config.scale_factor * (config.visuals.gizmo_size * 0.1 + config.visuals.stroke_width * 2.0))
        as f64
}

pub(crate) fn plane_local_origin(config: &PreparedGizmoConfig, direction: GizmoDirection) -> DVec3 {
    let offset = config.scale_factor * config.visuals.gizmo_size * 0.5;

    let a = plane_bitangent(direction);
    let b = plane_tangent(direction);
    (a + b) * offset as f64
}

pub(crate) fn plane_global_origin(
    config: &PreparedGizmoConfig,
    direction: GizmoDirection,
) -> DVec3 {
    let mut origin = plane_local_origin(config, direction);
    if config.local_space() {
        origin = config.rotation * origin;
    }
    origin + config.translation
}

pub(crate) fn plane_visibility(config: &PreparedGizmoConfig, direction: GizmoDirection) -> f64 {
    let dot = config
        .eye_to_model_dir
        .dot(gizmo_normal(config, direction))
        .abs();
    (1.0 - ((1.0 - dot) - *PLANE_FADE.start()) / (*PLANE_FADE.end() - *PLANE_FADE.start())).min(1.0)
}

/// Radius to use for inner circle subgizmos
pub(crate) fn inner_circle_radius(config: &PreparedGizmoConfig) -> f64 {
    (config.scale_factor * config.visuals.gizmo_size) as f64 * 0.2
}

/// Radius to use for outer circle subgizmos
pub(crate) fn outer_circle_radius(config: &PreparedGizmoConfig) -> f64 {
    (config.scale_factor * (config.visuals.gizmo_size + config.visuals.stroke_width + 5.0)) as f64
}

pub(crate) fn gizmo_local_normal(config: &PreparedGizmoConfig, direction: GizmoDirection) -> DVec3 {
    match direction {
        GizmoDirection::X => DVec3::X,
        GizmoDirection::Y => DVec3::Y,
        GizmoDirection::Z => DVec3::Z,
        GizmoDirection::View => -config.view_forward(),
    }
}

pub(crate) fn gizmo_normal(config: &PreparedGizmoConfig, direction: GizmoDirection) -> DVec3 {
    let mut normal = gizmo_local_normal(config, direction);

    if config.local_space() && direction != GizmoDirection::View {
        normal = config.rotation * normal;
    }

    normal
}

pub(crate) fn gizmo_color_solid(
    config: &PreparedGizmoConfig,
    focused: bool,
    direction: GizmoDirection,
) -> Color32 {
    let color = match direction {
        GizmoDirection::X => config.visuals.x_color,
        GizmoDirection::Y => config.visuals.y_color,
        GizmoDirection::Z => config.visuals.z_color,
        GizmoDirection::View => config.visuals.s_color,
    };

    if focused {
        config.visuals.highlight_color.unwrap_or(color)
    } else {
        color
    }
}

pub(crate) fn gizmo_color(
    config: &PreparedGizmoConfig,
    focused: bool,
    direction: GizmoDirection,
) -> Color32 {
    let alpha = if focused {
        config.visuals.highlight_alpha
    } else {
        config.visuals.inactive_alpha
    };

    gizmo_color_solid(config, focused, direction).linear_multiply(alpha)
}
