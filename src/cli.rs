//! Command-line surface. The bare `stepview <file>` form opens the GUI; subcommands are headless.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "stepview",
    version,
    about = "Fast native STEP viewer for macOS on Apple Silicon",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// STEP file to open in the GUI.
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// Chord tolerance in millimetres (default: derived from model size).
    #[arg(long, global = true)]
    pub tol: Option<f64>,

    /// Skip the on-disk tessellation cache.
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Do not decode colour/style entities.
    #[arg(long, global = true)]
    pub no_colors: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print schema, units, entity counts, product structure, bounding box and diagnostics.
    Info {
        file: PathBuf,
        /// Emit machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
        /// List every entity type with its count.
        #[arg(long)]
        types: bool,
        /// Dump one entity (by `#id`) as a parsed tree.
        #[arg(long, value_name = "#ID")]
        dump: Option<String>,
        /// Print the assembly tree.
        #[arg(long)]
        tree: bool,
        /// Stop after the product structure (skip B-rep extraction).
        #[arg(long)]
        no_geometry: bool,
        /// Also tessellate and report mesh statistics and diagnostics.
        #[arg(long)]
        mesh: bool,
    },
    /// Render the model to a PNG without opening a window.
    Render {
        file: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Image size as WIDTHxHEIGHT.
        #[arg(long, default_value = "1600x1200")]
        size: String,
        /// iso | top | front | right | back | left | bottom | <yaw>,<pitch> (degrees)
        #[arg(long, default_value = "iso")]
        view: String,
        /// Background colour as #rrggbb.
        #[arg(long, default_value = "#f2f2f4")]
        bg: String,
        /// Render only assembly nodes whose name or path contains this text (case-insensitive).
        #[arg(long)]
        only: Option<String>,
    },
    /// Export the tessellated model as glb, stl or obj (chosen by extension).
    Export {
        file: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Bake all instances into world space (single mesh).
        #[arg(long)]
        flatten: bool,
        /// Write ASCII STL / text glTF where applicable.
        #[arg(long)]
        ascii: bool,
    },
    /// Time each pipeline stage.
    Bench {
        file: PathBuf,
        #[arg(long, default_value_t = 5)]
        iters: u32,
        /// index | eager | structure | extract | tess | all
        #[arg(long, default_value = "all")]
        stage: String,
    },
    /// Inspect or clear the tessellation cache.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheAction {
    Info,
    Clear,
    Prune,
}

pub fn run() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Info { file, json, types, dump, tree, no_geometry, mesh }) => crate::commands::info::run(
            &file,
            &crate::commands::info::InfoOpts { json, types, dump, tree, no_geometry, mesh, colors: !cli.no_colors, params: crate::pipeline::LoadOpts::from_cli(cli.tol, cli.no_cache, cli.no_colors).params },
        ),
        Some(Command::Export { file, out, flatten, ascii }) => {
            let opts = crate::pipeline::LoadOpts::from_cli(cli.tol, cli.no_cache, cli.no_colors);
            crate::commands::export::run(&file, &out, flatten, ascii, &opts)
        }
        Some(Command::Bench { file, iters, stage }) => {
            let opts = crate::pipeline::LoadOpts::from_cli(cli.tol, cli.no_cache, cli.no_colors);
            crate::commands::bench::run(&file, iters, &stage, &opts)
        }
        Some(Command::Cache { action }) => crate::commands::cache::run(&action),
        Some(Command::Render { file, out, size, view, bg, only }) => {
            let opts = crate::pipeline::LoadOpts::from_cli(cli.tol, cli.no_cache, cli.no_colors);
            let ropts = crate::commands::render::RenderOpts {
                size: crate::commands::render::parse_size(&size)?,
                view,
                background: crate::commands::render::parse_color(&bg)?,
                only,
            };
            crate::commands::render::run(&file, &out, &ropts, &opts)
        }
        None => {
            let opts = crate::pipeline::LoadOpts::from_cli(cli.tol, cli.no_cache, cli.no_colors);
            crate::gui::run(cli.file, opts)
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("STEPVIEW_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(false).without_time().try_init();
}
