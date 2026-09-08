//! stepview: fast native STEP viewer.

mod app;
mod cli;
mod commands;
mod gui;
mod loader;
#[cfg(target_os = "macos")]
mod macos;
mod pipeline;

fn main() -> anyhow::Result<()> {
    cli::run()
}
