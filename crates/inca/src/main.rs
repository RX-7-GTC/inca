//! Inca — a free 3D modeling, animation and rendering application.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod anim_extras;
mod app;
mod component_editor;
mod edit_cmds;
mod hypershade;
mod hotkey_editor;
mod hotkey_help;
mod hik_ui;
mod hypergraph;
mod icons;
mod icons_svg;
mod marking;
mod menus;
mod mesh_tools;
mod options;
mod panels;
mod prefs;
mod project;
mod render_view;
mod rig_tools;
mod scripting;
mod test_hook;
mod theme;
mod timeline;
mod toolbar;
mod uv_editor;
mod viewport;
mod vp_ui;
mod windows;
mod workspace;

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../../build/icon256.png")).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

fn main() -> eframe::Result {
    prefs::apply_backend_env();
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1600.0, 1000.0]).with_min_inner_size([900.0, 600.0]).with_title("Inca");
    if let Some(i) = icon() {
        viewport = viewport.with_icon(i);
    }
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    // GUI/editor latency matters more than throughput here; don't queue two frames.
    wgpu_options.surface = eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY;
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, wgpu_options, ..Default::default() };
    eframe::run_native("Inca", options, Box::new(|cc| Ok(Box::new(app::IncaApp::new(cc)))))
}
