use crate::cli::CacheAction;

pub fn run(action: &CacheAction) -> anyhow::Result<()> {
    let dir = step_cache::default_dir().ok_or_else(|| anyhow::anyhow!("no cache directory available"))?;
    match action {
        CacheAction::Info => {
            let i = step_cache::info(&dir);
            println!("{}: {} files, {:.1} MB", i.dir.display(), i.files, i.bytes as f64 / 1e6);
        }
        CacheAction::Clear => {
            let n = step_cache::clear(&dir)?;
            println!("removed {n} cache files from {}", dir.display());
        }
        CacheAction::Prune => {
            let n = step_cache::prune(&dir, 2 * 1024 * 1024 * 1024)?;
            let i = step_cache::info(&dir);
            println!("removed {n} files; now {} files, {:.1} MB", i.files, i.bytes as f64 / 1e6);
        }
    }
    Ok(())
}
