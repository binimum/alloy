mod app;
mod audio;
mod config;
mod integrations;
mod library;
mod plugins;
mod theme;

use anyhow::Context;
use app::AlloyApp;

fn main() -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([920.0, 560.0])
            .with_title("Alloy"),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "Alloy",
        native_options,
        Box::new(|cc| {
            Ok(Box::new(
                AlloyApp::new(cc).context("failed to start Alloy")?,
            ))
        }),
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))
}
