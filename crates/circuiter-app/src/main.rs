//! Circuiter desktop application.

mod app;
mod canvas;

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "circuiter-gui", version, about = "Circuiter desktop application")]
pub struct Args {
    /// Project directory to open.
    pub project: Option<PathBuf>,
    /// Cell to open instead of the project's top cell.
    #[arg(long)]
    pub cell: Option<String>,
    /// Colour theme: `paper` or `bench`.
    #[arg(long)]
    pub theme: Option<String>,
    /// Select an instance by name after opening (useful with --screenshot).
    #[arg(long)]
    pub select: Option<String>,
    /// Render the window, save a PNG screenshot to this path, and exit.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Circuiter")
            .with_app_id("circuiter")
            .with_inner_size([1600.0, 1000.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Circuiter",
        options,
        Box::new(move |cc| Ok(Box::new(app::CircuiterApp::new(cc, args)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
