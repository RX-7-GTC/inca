use glam::DVec3;
use ecolor::Color32;
use std::ops::Add;

use crate::math::{round_to_interval, world_to_screen, Pos2, Vec2};

use crate::subgizmo::common::{
    arrow_end, draw_arrow, draw_gizmo_cube, draw_plane, gizmo_color_solid, gizmo_local_normal,
    inner_circle_radius, pick_arrow, pick_circle, pick_plane, plane_bitangent, plane_tangent,
};
use crate::subgizmo::{SubGizmoConfig, SubGizmoKind, common::TransformKind};
use crate::{GizmoDirection, GizmoDrawData, GizmoMode, GizmoResult, gizmo::Ray};

use super::common::PickResult;

pub(crate) type ScaleSubGizmo = SubGizmoConfig<Scale>;

#[derive(Debug, Copy, Clone, Hash)]
pub(crate) struct ScaleParams {
    pub mode: GizmoMode,
    pub direction: GizmoDirection,
    pub transform_kind: TransformKind,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct ScaleState {
    start_delta: f64,
    sign: f64,
    axis_dir: Vec2,
}

impl Default for ScaleState {
    fn default() -> Self {
        Self { start_delta: 1.0, sign: 1.0, axis_dir: Vec2::X }
    }
}

#[derive(Default, Debug, Copy, Clone)]
pub(crate) struct Scale;

impl SubGizmoKind for Scale {
    type Params = ScaleParams;
    type State = ScaleState;
    type PickPreview = PickResult;

    fn pick_preview(subgizmo: &SubGizmoConfig<Self>, ray: Ray) -> super::common::PickResult
    where
        Self: Sized,
    {
        match (subgizmo.transform_kind, subgizmo.direction) {
            (TransformKind::Plane, GizmoDirection::View) => pick_circle(
                &subgizmo.config,
                ray,
                inner_circle_radius(&subgizmo.config),
                true,
            ),
            (TransformKind::Plane, _) => pick_plane(&subgizmo.config, ray, subgizmo.direction),
            (TransformKind::Axis, _) => {
                pick_arrow(&subgizmo.config, ray, subgizmo.direction, subgizmo.mode)
            }
        }
    }

    fn pick(subgizmo: &mut ScaleSubGizmo, ray: Ray) -> Option<f64> {
        let pick_result = Self::pick_preview(subgizmo, ray);

        let axis_dir = axis_screen_dir(subgizmo).unwrap_or(Vec2::X);
        subgizmo.state.axis_dir = axis_dir;
        let start_delta = match (subgizmo.transform_kind, subgizmo.direction) {
            // Uniform scale reads horizontal cursor movement from the press point.
            (TransformKind::Plane, GizmoDirection::View) => ray.screen_pos.x as f64,
            // Axes and plane handles both use the signed projection along their locked handle
            // direction, so both can be dragged through the pivot into negative scale.
            _ => signed_distance_with(subgizmo, ray.screen_pos, axis_dir)?,
        };

        subgizmo.state.start_delta = start_delta;
        subgizmo.state.sign = if start_delta < 0.0 { -1.0 } else { 1.0 };

        if pick_result.picked {
            Some(pick_result.t)
        } else {
            None
        }
    }

    fn update(subgizmo: &mut ScaleSubGizmo, ray: Ray) -> Option<GizmoResult> {
        let view_plane = subgizmo.transform_kind == TransformKind::Plane
            && subgizmo.direction == GizmoDirection::View;
        let mut delta = if view_plane {
            // Uniform scale follows the horizontal drag: left shrinks, right grows.
            uniform_scale_factor(subgizmo, ray.screen_pos)?
        } else {
            signed_scale_ratio(subgizmo, ray.screen_pos)?
        };

        if subgizmo.config.snapping {
            delta = round_to_interval(delta, subgizmo.config.snap_scale as f64);
        }
        delta -= 1.0;

        let direction = match (subgizmo.transform_kind, subgizmo.direction) {
            (TransformKind::Axis, _) => gizmo_local_normal(&subgizmo.config, subgizmo.direction),
            (TransformKind::Plane, GizmoDirection::View) => DVec3::ONE,
            // Both free axes take the exact drag ratio. Keeping the sum unnormalized makes the
            // scale reach zero at the pivot instead of 1 - 1/sqrt(2).
            (TransformKind::Plane, _) => {
                plane_bitangent(subgizmo.direction) + plane_tangent(subgizmo.direction)
            }
        };

        let scale = DVec3::ONE + (direction * delta);

        Some(GizmoResult::Scale {
            total: scale.into(),
        })
    }

    fn draw(subgizmo: &ScaleSubGizmo) -> GizmoDrawData {
        match (subgizmo.transform_kind, subgizmo.direction) {
            (TransformKind::Axis, _) => {
                let mut draw_data = draw_arrow(
                    &subgizmo.config,
                    subgizmo.focused,
                    subgizmo.direction,
                    subgizmo.mode,
                );
                let end = arrow_end(&subgizmo.config, subgizmo.direction, subgizmo.mode);
                let center = if subgizmo.config.local_space() {
                    subgizmo.config.rotation * end + subgizmo.config.translation
                } else {
                    end + subgizmo.config.translation
                };
                let color = gizmo_color_solid(
                    &subgizmo.config,
                    subgizmo.focused || subgizmo.active,
                    subgizmo.direction,
                );
                draw_data = draw_data.add(draw_gizmo_cube(&subgizmo.config, center, 5.2, color));
                draw_data
            }
            (TransformKind::Plane, GizmoDirection::View) => {
                let color = if subgizmo.active || subgizmo.focused {
                    Color32::from_rgb(255, 235, 80)
                } else {
                    Color32::from_rgb(0, 200, 200)
                };
                draw_gizmo_cube(&subgizmo.config, subgizmo.config.translation, 5.2, color)
            }
            (TransformKind::Plane, _) => {
                draw_plane(&subgizmo.config, subgizmo.focused, subgizmo.direction)
            }
        }
    }
}

/// Signed drag ratio along the handle's locked screen direction. The 2 px deadband keeps the
/// previous side near the pivot, so the sign does not flicker while crossing zero.
fn signed_scale_ratio(subgizmo: &mut ScaleSubGizmo, cursor_pos: Pos2) -> Option<f64> {
    let mut signed = signed_distance_with(subgizmo, cursor_pos, subgizmo.state.axis_dir)?;
    let deadband = 2.0;
    if signed > deadband {
        subgizmo.state.sign = 1.0;
    } else if signed < -deadband {
        subgizmo.state.sign = -1.0;
    }
    if signed.abs() < deadband {
        signed = subgizmo.state.sign * deadband;
    }
    Some(signed / subgizmo.state.start_delta)
}

/// Screen direction of the handle at pick time (the axis, or the diagonal of a plane handle).
/// Locking it keeps the signed mapping stable after the object scale turns negative.
fn axis_screen_dir(subgizmo: &ScaleSubGizmo) -> Option<Vec2> {
    let viewport = subgizmo.config.viewport;
    let origin = world_to_screen(viewport, subgizmo.config.mvp, DVec3::ZERO)?;
    let local_axis = match (subgizmo.transform_kind, subgizmo.params.direction) {
        (_, GizmoDirection::View) => return None,
        (TransformKind::Axis, GizmoDirection::X) => DVec3::X,
        (TransformKind::Axis, GizmoDirection::Y) => DVec3::Y,
        (TransformKind::Axis, GizmoDirection::Z) => DVec3::Z,
        (TransformKind::Plane, d) => plane_bitangent(d) + plane_tangent(d),
    };
    let tip = world_to_screen(viewport, subgizmo.config.mvp, local_axis)?;
    Some((tip - origin).normalized())
}

fn signed_distance_with(subgizmo: &ScaleSubGizmo, cursor_pos: Pos2, dir: Vec2) -> Option<f64> {
    let origin = world_to_screen(subgizmo.config.viewport, subgizmo.config.mvp, DVec3::ZERO)?;
    Some((cursor_pos - origin).dot(dir) as f64)
}

/// Uniform scale from horizontal cursor movement. Dragging one unit needs
/// `UNIFORM_DRAG_RADII_PER_UNIT` gizmo radii of travel; `start_delta` holds the press x.
/// The factor is signed, so dragging through the pivot flips the object like Maya; a small
/// deadband keeps the sign from flickering at the crossing.
fn uniform_scale_factor(subgizmo: &mut ScaleSubGizmo, cursor_pos: Pos2) -> Option<f64> {
    const UNIFORM_DRAG_RADII_PER_UNIT: f64 = 2.0;
    const DEADBAND_PX: f64 = 2.0;
    let reference = (subgizmo.config.visuals.gizmo_size as f64 * UNIFORM_DRAG_RADII_PER_UNIT).max(1.0);
    // Pixels past the zero crossing (factor 1 at press, 0 after `reference` px to the left).
    let mut d = cursor_pos.x as f64 - subgizmo.state.start_delta + reference;
    if d > DEADBAND_PX {
        subgizmo.state.sign = 1.0;
    } else if d < -DEADBAND_PX {
        subgizmo.state.sign = -1.0;
    }
    if d.abs() < DEADBAND_PX {
        d = subgizmo.state.sign * DEADBAND_PX;
    }
    Some(d / reference)
}
