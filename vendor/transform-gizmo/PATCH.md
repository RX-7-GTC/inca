# Local patch: transform-gizmo 0.11.0

Vendored from the crates.io `transform-gizmo` 0.11.0 source (MIT OR Apache-2.0).

Only the active rotation gizmo's drawing is changed, to match the Maya / old JS Inca
behaviour:

- Draw a dimmed full ring plus a bright swept arc instead of a filled sector.
- Wrap the swept angle to ±180° so the arc flips to the short side instead of winding
  over itself.
- Do not increase the fill opacity with full turns (it used to become opaque after a
  few spins).

`src/subgizmo/rotation.rs` contains the patch. `src/shape.rs` only marks the two shape
helpers that are no longer used by the rotation code as `#[allow(dead_code)]`.

The dependency is wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.
