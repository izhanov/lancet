use std::path::PathBuf;
use std::time::Instant;

use anyhow::bail;
use lancet_lib::index::{Indexer, Options};

use super::{Global, pretty_path};

#[derive(clap::Args)]
pub struct IndexArgs {
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// Re-index even unchanged files (skip the mtime check)
    #[arg(short, long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct ReindexArgs {
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}

pub async fn run(g: &Global, roots: &[PathBuf], reset: bool, force: bool) -> anyhow::Result<()> {
    let start = Instant::now();
    let store = g.open_store().await?;
    // Before the reset wipes the model the database was built with.
    let embedder = g.embedder(&store).await?;
    if reset {
        store.reset().await?;
    }

    let indexer = Indexer::new(g.parsers()?, embedder);
    let report = |path: &str, chunks: usize| {
        if !g.quiet {
            println!("  {} — {chunks} chunks indexed", pretty_path(path));
        }
    };
    let sum = indexer
        .run(
            &store,
            roots,
            &Options {
                force,
                on_file: Some(&report),
            },
        )
        .await?;

    if sum.files == 0 && sum.failed == 0 {
        println!(
            "nothing to do: {} files unchanged, {} stale removed",
            sum.skipped, sum.removed
        );
        return Ok(());
    }

    let stored = store.count().await?;
    print!("{} chunks indexed, {stored} in the database", sum.chunks);
    if sum.skipped > 0 || sum.removed > 0 {
        print!(
            " ({} unchanged skipped, {} stale removed)",
            sum.skipped, sum.removed
        );
    }
    println!();
    tracing::info!(
        files = sum.files,
        failed = sum.failed,
        chunks = sum.chunks,
        skipped = sum.skipped,
        removed = sum.removed,
        took = ?start.elapsed(),
        "done"
    );

    if sum.failed > 0 {
        bail!(
            "{} of {} files failed to index",
            sum.failed,
            sum.files + sum.failed
        );
    }
    Ok(())
}
