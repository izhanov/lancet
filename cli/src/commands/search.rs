use std::time::Instant;

use lancet_lib::search::{self, Mode, Options};
use lancet_lib::store::SearchResult;

use super::{Global, colors_enabled, paint, pretty_path, snippet};

#[derive(clap::Args)]
pub struct Args {
    /// The query; quote it when it carries -word exclusions
    #[arg(required = true, value_name = "QUERY")]
    query: Vec<String>,
    /// Maximum number of results
    #[arg(long, default_value_t = 5)]
    limit: usize,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
    /// Search mode: hybrid, vector (by meaning) or text (by words)
    #[arg(long, default_value = "hybrid")]
    mode: Mode,
    /// Drop results whose file path contains this fragment (repeatable)
    #[arg(long, value_name = "FRAGMENT")]
    exclude_path: Vec<String>,
}

pub async fn run(g: &Global, args: Args) -> anyhow::Result<()> {
    let start = Instant::now();
    let query = args.query.join(" ");
    let store = g.open_store().await?;
    // Text mode works without an embedder (and on a foreign-model
    // database), so only build one when the mode needs vectors.
    let embedder = (args.mode != Mode::Text).then(|| g.embedder());

    let opts = Options {
        mode: args.mode,
        limit: args.limit,
        exclude: Vec::new(),
        exclude_path: args.exclude_path,
    };
    let mut results = search::search(&store, embedder.as_deref(), query.trim(), &opts).await?;
    tracing::debug!(mode = %args.mode, results = results.len(), took = ?start.elapsed(), "search done");

    if args.json {
        for r in &mut results {
            r.score = (r.score * 10_000.0).round() / 10_000.0;
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    print_results(&results);
    Ok(())
}

fn print_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("nothing found");
        return;
    }
    let color = colors_enabled();
    for (i, r) in results.iter().enumerate() {
        let score = format!("{:.2}", r.score);
        let code = match r.score {
            s if s >= 0.7 => "32", // green
            s if s >= 0.5 => "33", // yellow
            _ => "90",             // gray
        };
        println!(
            "{:<2} {}  {}",
            i + 1,
            paint(color, code, &score),
            paint(color, "1", &pretty_path(&r.path))
        );
        if !r.section.is_empty() {
            println!("         {}", paint(color, "2", &r.section));
        }
        println!("         {}\n", snippet(&r.text, 200));
    }
}
