//! Circuiter desktop application.

mod app;
mod canvas;
mod edit;
#[cfg(test)]
mod gui_tests;
mod import;
mod interface;
mod plot;
mod simulation;
mod style;
mod symbol_edit;
mod workspace;

use clap::Parser;
use std::path::PathBuf;

/// Options for opening the app.
#[derive(Debug, Parser)]
#[command(name = "circuiter", version, about = "Circuiter: design, simulate and build hardware")]
pub struct Args {
    /// Project directory to open.
    #[arg(value_name = "PROJECT")]
    pub project: Option<PathBuf>,
    /// Cell to open instead of the project's top cell.
    #[arg(long)]
    pub cell: Option<String>,
    /// Colour theme: `paper` or `bench`.
    #[arg(long)]
    pub theme: Option<String>,
    /// View to show: `schematic` or `symbol`.
    #[arg(long)]
    pub view: Option<String>,
    /// Select an instance (or, in the symbol view, a pin) by name after opening.
    #[arg(long)]
    pub select: Option<String>,
    /// Run an analysis after opening: `op`, `tran` (or `tran:STOP`, e.g. `tran:3`) or `ac`.
    #[arg(long)]
    pub simulate: Option<String>,
    /// After a transient run, plot part temperatures rather than signals.
    #[arg(long)]
    pub heat: bool,
    /// Render the window, save a PNG screenshot to this path, and exit.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    run_app(Args::parse())
}

fn run_app(args: Args) -> anyhow::Result<()> {
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
