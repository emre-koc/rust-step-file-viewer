//! GUI entry point.

use std::path::PathBuf;

use anyhow::Context;

use crate::pipeline::LoadOpts;

pub fn run(initial: Option<PathBuf>, opts: LoadOpts) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    crate::macos::install_open_handler();
    // eframe otherwise replaces the bundle's Dock icon with its own default logo.
    // Share the source used by macos/make_icon.sh so both launch paths look identical.
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../macos/icon_source.png"))
        .context("decode the bundled StepView icon")?;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("StepView")
            .with_icon(icon)
            .with_app_id("com.dresden.stepview")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([800.0, 500.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration::default(),
        persist_window: true,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "StepView",
        native,
        Box::new(move |cc| Ok(Box::new(crate::app::App::new(cc, initial, opts)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
