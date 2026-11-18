# Local patch: egui 0.36.2

Vendored from the crates.io `egui` 0.36.2 source (MIT OR Apache-2.0).

Only the colour picker's numeric row is changed:

- The `R` / `G` / `B` (and `A`) fields and the U8/F toggle use fixed widths, so switching
  between `U8` and `0-1` (`F`) mode no longer resizes the row. Inca draws a HEX field on the
  same row at a fixed offset, which needs a stable row width.
- The saturation/value marker circle has a fixed 14 px radius instead of `width / 12`.
- `Scene::default` silences the edition-2024 `f32::EPSILON` deprecation warning.

The dependency is wired in through `[patch.crates-io]` in the workspace `Cargo.toml`.
