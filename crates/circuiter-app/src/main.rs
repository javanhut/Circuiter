//! Circuiter desktop application.

mod app;
mod canvas;
mod cosim;
mod digital;
mod edit;
mod glow;
#[cfg(test)]
mod gui_tests;
mod import;
mod interface;
mod live;
mod mcp;
mod plot;
mod render3d;
mod simulation;
mod style;
mod symbol_edit;
mod toolchain;
mod watch;
mod workspace;

use clap::Parser;
use std::path::PathBuf;

/// Options for opening the app.
#[derive(Debug, Parser)]
#[command(name = "circuiter", version, about = "Circuiter: design, simulate and build hardware")]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Project directory to open.
    #[arg(value_name = "PROJECT")]
    pub project: Option<PathBuf>,
    /// Cell to open instead of the project's top cell.
    #[arg(long)]
    pub cell: Option<String>,
    /// Colour theme: `paper` or `bench`.
    #[arg(long)]
    pub theme: Option<String>,
    /// View to show: `schematic`, `symbol`, `diagram` (the sheet laid out
    /// automatically), `pcb`, `3d`, or `board3d` (the laid-out board in 3D).
    #[arg(long)]
    pub view: Option<String>,
    /// Select an instance (or, in the symbol view, a pin; in the 3D view, a feature of
    /// the body) by name after opening.
    #[arg(long)]
    pub select: Option<String>,
    /// Run an analysis after opening: `op`, `tran` (or `tran:STOP`, e.g. `tran:3`) or `ac`.
    #[arg(long)]
    pub simulate: Option<String>,
    /// After a transient run, plot part temperatures rather than signals.
    #[arg(long)]
    pub heat: bool,
    /// Start the circuit running live after opening.
    #[arg(long)]
    pub live: bool,
    /// Show this tab of the results panel: waveforms, thermal, problems or oscilloscope
    /// (or `none` to hide the panel).
    #[arg(long)]
    pub results: Option<String>,
    /// Open a program (relative to the project) in the code editor.
    #[arg(long)]
    pub program: Option<PathBuf>,
    /// Render the window, save a PNG screenshot to this path, and exit.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
}

/// What to do instead of opening the window.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Serve the Model Context Protocol on stdin and stdout, so an AI assistant can work
    /// on a project: make components, draw and wire schematics, simulate, lay out boards.
    /// Register it with the assistant, for Claude Code:
    /// `claude mcp add circuiter -- circuiter mcp /path/to/project`.
    Mcp {
        /// The project to work on (default: the one the current directory is in, if
        /// any; the assistant can also open or create one).
        #[arg(value_name = "PROJECT")]
        project: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let mut args = Args::parse();
    match args.command.take() {
        Some(Command::Mcp { project }) => mcp::serve(project),
        None => run_app(args),
    }
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
