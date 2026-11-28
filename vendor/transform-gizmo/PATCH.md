# Local patch: transform-gizmo 0.11.0

Vendored from the crates.io `transform-gizmo` 0.11.0 source (MIT OR Apache-2.0).

Only the active rotation gizmo's drawing is changed, to match the Maya / old JS Inca
behaviour:

- Draw a dimmed full ring plus a bright swept arc instead of a filled sector.
- Wrap the swept angle to ±180° so the arc flips to the short side instead of winding
  over itself.
- Do not increase the fill opacity with full turns (it used to become opaque after a
  few spins).
- Arcball free rotate: raise the angular response to 8x the stock rate and reverse the axis to
  match Maya's trackball direction.
- Uniform scale: pick only the small centre area instead of the whole outer ring. The centre is drawn
  as a cube, cyan normally and yellow when active.
- Scale axis tips are drawn as small 3D cubes using the gizmo’s own projection, with the shaft
  ending at the cube centre. The cubes use the solid axis colour (not the inactive-alpha blend)
  and draw only front faces, so they match the shaft brightness.
- Scale cube faces seen edge-on (projected area under 2 px²) are skipped. epaint’s feathering turns
  such a degenerate quad into a thin line across the whole viewport.
- Scale axis drags use a signed screen projection, so dragging past the gizmo centre inverts the
  scale like Maya instead of stopping at zero. A 2 px deadband keeps the sign from flickering at
  the crossing.
- Plane handles use the same signed projection along their screen diagonal, so they cross the
  pivot into negative scale too. Both free axes take the exact ratio; the diagonal vector is no
  longer normalized (1 - 1/sqrt(2) used to remain at the pivot).
- Translate arrows end in a small 3D cone drawn as raw triangles (Maya-style), with the shaft
  stopping at the cone base. The cone is about 10 px across and 20 px long (at most 40 % of a
  very short arrow) and its faces use the flat, unshaded solid axis colour, like Maya's; the raw mesh avoids epaint's feathering, which
  turns the sharp near-edge-on triangles into spikes.
- Uniform scale (centre cube) follows horizontal cursor movement from the press point: dragging
  left shrinks, right grows. Two gizmo radii of drag is one full unit; dragging through the pivot
  flips the object to negative scale, with a 2 px deadband at the crossing.
- Plane handles that project larger than 120 px are culled; near-edge-on views could otherwise
  stretch a plane across the viewport.
- `GizmoVisuals::shaft_width` draws the translate shafts thinner than `stroke_width` (Inca's
  Move tool: 1 px, like Maya) without changing handle sizes, arrow lengths or the pick distance.
- Idle plane handles are drawn at 60 % opacity, like Maya's see-through plane squares, even
  when the axis lines are fully opaque (Inca's Move tool sets the inactive alpha to 1).

`src/subgizmo/rotation.rs`, `src/subgizmo/scale.rs` and `src/subgizmo/arcball.rs` contain the patches. `src/shape.rs` only marks the two shape
helpers that are no longer used by the rotation code as `#[allow(dead_code)]`.

The dependency is wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.
