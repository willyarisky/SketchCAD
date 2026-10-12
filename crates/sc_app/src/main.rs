//! SketchCAD — a SketchUp-style 3D modeler.

// No console window behind the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod demo;
mod document;
mod groups;
mod inference;
mod plugins;
mod scene;
mod scenes;
mod skp;
mod tools;
mod ui;
#[cfg(test)]
mod ui_tests;

fn main() -> eframe::Result {
    env_logger::init();
    let opts = app::LaunchOptions::from_args(std::env::args_os().skip(1));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Untitled - SketchCAD").with_inner_size([1440.0, 900.0]).with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("SketchCAD", options, Box::new(|cc| Ok(Box::new(app::SketchCadApp::new(cc, opts)))))
}
