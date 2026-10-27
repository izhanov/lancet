use std::path::PathBuf;

use lancet_lib::eval::{self, Score};
use lancet_lib::search::{self, Mode, Options};

use super::{Global, colors_enabled, paint, pretty_path, snippet};

#[derive(clap::Args)]
pub struct Args {
    /// Golden sets to run; none runs every set in the evals directory
    #[arg(value_name = "GOLDEN.yaml")]
    files: Vec<PathBuf>,
    /// Results per query; recall is measured at this depth
    #[arg(long, default_value_t = 5)]
    limit: usize,
    /// Search mode to evaluate: hybrid, vector or text
    #[arg(long, default_value = "hybrid")]
    mode: Mode,
}

pub async fn run(g: &Global, args: Args) -> anyhow::Result<()> {
    let files = if args.files.is_empty() {
        eval::golden_files(&g.evals)?
    } else {
        args.files
    };
    let store = g.open_store().await?;
    let embedder = match args.mode {
        Mode::Text => None,
        _ => Some(g.embedder(&store).await?),
    };
    let opts = Options {
        mode: args.mode,
        limit: args.limit,
        ..Options::default()
    };

    let color = colors_enabled();
    let mut total = Score::default();
    for path in &files {
        let cases = eval::load_golden(path)?;
        if files.len() > 1 {
            println!(
                "{}",
                paint(color, "1", &pretty_path(&path.to_string_lossy()))
            );
        }

        let mut score = Score::default();
        for (i, case) in cases.iter().enumerate() {
            let results = search::search(&store, embedder.as_deref(), &case.query, &opts)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("{} case {} ({:?}): {e}", path.display(), i + 1, case.query)
                })?;

            let found = eval::first_expected_rank(&results, &case.expect);
            score.add(found.map(|(rank, _)| rank));
            match found {
                Some((rank, found)) => {
                    let hit = paint(color, "32", &format!("hit@{rank}"));
                    println!("{:<2} {hit}  {}", i + 1, snippet(&case.query, 60));
                    println!("          {}", paint(color, "2", &pretty_path(found)));
                }
                None => {
                    println!(
                        "{:<2} {}   {}",
                        i + 1,
                        paint(color, "31", "miss"),
                        snippet(&case.query, 60)
                    );
                    println!(
                        "          {}",
                        paint(color, "2", &format!("want {}", case.expect.join(", ")))
                    );
                }
            }
        }

        println!(
            "\nrecall@{} {:.2} ({}/{})   MRR {:.3}   mode={}\n",
            args.limit,
            score.recall(),
            score.hits,
            score.cases,
            score.mrr(),
            args.mode
        );
        total.merge(score);
    }

    if files.len() > 1 {
        println!(
            "overall: recall@{} {:.2} ({}/{})   MRR {:.3}",
            args.limit,
            total.recall(),
            total.hits,
            total.cases,
            total.mrr()
        );
    }
    Ok(())
}
