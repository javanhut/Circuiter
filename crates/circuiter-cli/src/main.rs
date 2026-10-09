//! `circuiter`: headless command-line interface.

use anyhow::{Context, Result, bail};
use circuiter_model::check::{Severity, check_libraries};
use circuiter_model::connectivity::{Connectivity, Item};
use circuiter_model::{CellId, CellRef, Design, HierNode, ParamKind, Project};
use circuiter_scene::{Scene, Theme, to_svg};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(name = "circuiter", version, about = "Circuiter: design, simulate and build hardware")]
struct Cli {
    /// Project directory (default: the project containing the current directory).
    #[arg(short = 'C', long = "project", global = true)]
    project: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new project.
    New {
        /// Directory to create the project in.
        path: PathBuf,
        /// Project name (default: derived from the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Check the project for structural errors.
    Check,
    /// Rewrite the project's files in canonical form.
    Fmt,
    /// Print the elaborated instance hierarchy.
    Tree {
        /// Cell to start from (default: the project's top cell).
        cell: Option<String>,
    },
    /// List the nets of a cell's schematic and anything left unconnected.
    Nets {
        /// Cell to analyse (default: the project's top cell).
        cell: Option<String>,
    },
    /// Render a cell's schematic to SVG.
    Render {
        /// Cell to render (default: the project's top cell).
        cell: Option<String>,
        /// Output file.
        #[arg(short, long)]
        output: PathBuf,
        /// Colour theme: `paper` or `bench`.
        #[arg(long, default_value = "paper")]
        theme: String,
    },
    /// Browse libraries.
    Lib {
        #[command(subcommand)]
        command: LibCommand,
    },
}

#[derive(Debug, Subcommand)]
enum LibCommand {
    /// List libraries, or the cells in one library.
    List { library: Option<String> },
    /// Show a cell's ports, parameters and views, e.g. `std:nmos`.
    Show { cell: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::New { path, name } => cmd_new(&path, name.as_deref()),
        Command::Check => cmd_check(&open_project(cli.project.as_deref())?),
        Command::Fmt => {
            let project = open_project(cli.project.as_deref())?;
            project.save()?;
            println!("Formatted {}", project.root.display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Tree { cell } => cmd_tree(&open_project(cli.project.as_deref())?, cell.as_deref()),
        Command::Nets { cell } => cmd_nets(&open_project(cli.project.as_deref())?, cell.as_deref()),
        Command::Render { cell, output, theme } => {
            cmd_render(&open_project(cli.project.as_deref())?, cell.as_deref(), &output, &theme)
        }
        Command::Lib { command } => {
            // Library browsing works outside a project, with just the standard library.
            let design = match find_project(cli.project.as_deref()) {
                Some(root) => load_design(&Project::load(&root)?)?,
                None => standalone_design()?,
            };
            match command {
                LibCommand::List { library } => cmd_lib_list(&design, library.as_deref()),
                LibCommand::Show { cell } => cmd_lib_show(&design, &cell),
            }
        }
    }
}

fn find_project(explicit: Option<&Path>) -> Option<PathBuf> {
    let start = match explicit {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    Project::find_root(&start)
}

fn open_project(explicit: Option<&Path>) -> Result<Project> {
    let Some(root) = find_project(explicit) else {
        bail!(
            "no {} found here or in any parent directory; create a project with `circuiter new <dir>`",
            circuiter_model::project::MANIFEST_FILE
        );
    };
    Ok(Project::load(&root)?)
}

fn load_design(project: &Project) -> Result<Design> {
    Ok(Design::for_project(project, circuiter_stdlib::load()?)?)
}

fn standalone_design() -> Result<Design> {
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::load()?)?;
    Ok(design)
}

fn cmd_new(path: &Path, name: Option<&str>) -> Result<ExitCode> {
    let name = match name {
        Some(n) => n.to_owned(),
        None => name_from_dir(path)?,
    };
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    let project = Project::create(path, &name)?;
    println!("Created project `{}` in {}", project.name(), path.display());
    Ok(ExitCode::SUCCESS)
}

/// Turns a directory name like `my-amp 2` into a valid project name like `my_amp_2`.
fn name_from_dir(path: &Path) -> Result<String> {
    let base = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("cannot derive a project name from the path; pass --name")?;
    let mut name: String =
        base.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name.insert_str(0, "p_");
    }
    Ok(name)
}

fn cmd_check(project: &Project) -> Result<ExitCode> {
    let design = load_design(project)?;
    let mut libraries: Vec<&str> = vec![project.name()];
    libraries.extend(project.manifest.libraries.keys().map(String::as_str));
    let mut diagnostics = check_libraries(&design, &libraries);

    let top = CellId::new(project.name(), &project.manifest.project.top);
    if design.cell(&top).is_none() {
        diagnostics.push(circuiter_model::check::Diagnostic {
            severity: Severity::Error,
            location: circuiter_model::project::MANIFEST_FILE.to_owned(),
            message: format!("top cell `{top}` does not exist"),
        });
    }

    for d in &diagnostics {
        println!("{d}");
    }
    let errors = diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = diagnostics.len() - errors;
    let cells: usize =
        libraries.iter().filter_map(|l| design.library(l)).map(|l| l.cells.len()).sum();
    println!("checked {cells} cells: {errors} error(s), {warnings} warning(s)");
    Ok(if errors > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Resolves a cell argument, defaulting to the project's top cell.
fn cell_arg(design: &Design, project: &Project, cell: Option<&str>) -> Result<CellId> {
    Ok(match cell {
        Some(c) => {
            let r: CellRef = c.parse().map_err(anyhow::Error::msg)?;
            design.resolve(&r, project.name()).with_context(|| format!("cell `{c}` not found"))?.0
        }
        None => CellId::new(project.name(), &project.manifest.project.top),
    })
}

fn cmd_nets(project: &Project, cell: Option<&str>) -> Result<ExitCode> {
    let design = load_design(project)?;
    let id = cell_arg(&design, project, cell)?;
    let def = design.cell(&id).with_context(|| format!("cell `{id}` not found"))?;
    let schematic = def.schematic.as_ref().with_context(|| format!("{id} has no schematic"))?;
    let conn = Connectivity::analyze(&design, &id.library, schematic);

    for net in &conn.nets {
        let mut members: Vec<String> = net
            .items
            .iter()
            .filter_map(|item| match *item {
                Item::Pin(i) => {
                    let pin = &conn.pins[i];
                    Some(format!("{}.{}", schematic.instances[pin.instance].name, pin.port))
                }
                Item::Port(i) => Some(format!("port:{}", schematic.ports[i].port)),
                _ => None,
            })
            .collect();
        members.sort();
        let conflict = if net.names.len() > 1 {
            format!("  (conflicting names: {})", net.names.join(", "))
        } else {
            String::new()
        };
        println!("{:<10} {}{conflict}", net.name, members.join(" "));
    }
    for &i in &conn.dangling_pins {
        let pin = &conn.pins[i];
        let inst = &schematic.instances[pin.instance].name;
        println!("unconnected pin: {inst}.{} at {}", pin.port, pin.at);
    }
    for p in &conn.dangling_wire_ends {
        println!("unconnected wire end at {p}");
    }
    println!(
        "{} net(s), {} junction(s), {} unconnected pin(s), {} unconnected wire end(s)",
        conn.nets.len(),
        conn.junctions.len(),
        conn.dangling_pins.len(),
        conn.dangling_wire_ends.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_render(
    project: &Project,
    cell: Option<&str>,
    output: &Path,
    theme: &str,
) -> Result<ExitCode> {
    let design = load_design(project)?;
    let id = cell_arg(&design, project, cell)?;
    let theme = Theme::ALL
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(theme))
        .with_context(|| format!("unknown theme `{theme}`; use paper or bench"))?;
    let scene = Scene::build(&design, &id).with_context(|| format!("{id} has no schematic"))?;
    std::fs::write(output, to_svg(&scene, theme))
        .with_context(|| format!("writing {}", output.display()))?;
    println!("Rendered {id} to {}", output.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_tree(project: &Project, cell: Option<&str>) -> Result<ExitCode> {
    let design = load_design(project)?;
    let top = cell_arg(&design, project, cell)?;
    let tree = design.hierarchy(&top)?;
    print_tree(&design, &tree, "", true, true);
    println!("{} leaf instance(s)", tree.leaf_count());
    Ok(ExitCode::SUCCESS)
}

fn print_tree(design: &Design, node: &HierNode, prefix: &str, last: bool, root: bool) {
    let global = design.cell(&node.cell).and_then(|d| d.cell.global_net.as_deref());
    let view = match (node.view, global) {
        (Some(v), _) => v.to_string(),
        (None, Some(net)) => format!("global net {net}"),
        (None, None) => "no view".to_owned(),
    };
    let label = match &node.instance {
        Some(inst) => format!("{inst}  {} ({view})", node.cell),
        None => format!("{} ({view})", node.cell),
    };
    let (branch, child_prefix) = match (root, last) {
        (true, _) => ("", String::new()),
        (false, true) => ("└── ", format!("{prefix}    ")),
        (false, false) => ("├── ", format!("{prefix}│   ")),
    };
    println!("{prefix}{branch}{label}");
    for (i, child) in node.children.iter().enumerate() {
        print_tree(design, child, &child_prefix, i + 1 == node.children.len(), false);
    }
}

fn cmd_lib_list(design: &Design, library: Option<&str>) -> Result<ExitCode> {
    match library {
        None => {
            for lib in design.libraries() {
                println!("{:<16} {} cell(s)", lib.name, lib.cells.len());
            }
        }
        Some(name) => {
            let lib = design.library(name).with_context(|| format!("no library `{name}`"))?;
            for def in lib.cells.values() {
                println!("{:<16} {}", def.name, def.cell.description);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_lib_show(design: &Design, cell: &str) -> Result<ExitCode> {
    let r: CellRef = cell.parse().map_err(anyhow::Error::msg)?;
    let context = design.project_library.as_deref().unwrap_or(circuiter_model::STD_LIBRARY);
    let (id, def) =
        design.resolve(&r, context).with_context(|| format!("cell `{cell}` not found"))?;

    println!("{id}");
    if !def.cell.description.is_empty() {
        println!("  {}", def.cell.description);
    }
    let views: Vec<&str> = [
        def.symbol.as_ref().map(|_| "symbol"),
        def.schematic.as_ref().map(|_| "schematic"),
        def.spice.as_ref().map(|_| "spice"),
    ]
    .into_iter()
    .flatten()
    .collect();
    println!("  views: {}", views.join(", "));
    if let Some(net) = &def.cell.global_net {
        println!("  global net: {net}");
    }
    println!("  ports:");
    for p in &def.cell.ports {
        println!("    {:<10} {:<14} {}", p.name, format!("{:?}", p.kind), p.description);
    }
    if !def.cell.params.is_empty() {
        println!("  parameters:");
        for p in &def.cell.params {
            let unit = match &p.kind {
                ParamKind::Real { unit } => unit.clone(),
                other => format!("{other:?}"),
            };
            let default = p.default.as_deref().unwrap_or("(required)");
            println!("    {:<10} {:<10} {:<8} {}", p.name, default, unit, p.description);
        }
    }
    Ok(ExitCode::SUCCESS)
}
