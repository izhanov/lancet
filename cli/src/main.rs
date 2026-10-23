//! `lt` — local semantic search over your files.

mod commands;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use commands::Global;

#[derive(Parser)]
#[command(name = "lt", version, about = "Semantic search over local files")]
struct Cli {
    #[command(flatten)]
    global: Global,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index files and directories
    Index(commands::index::IndexArgs),
    /// Wipe the database and index from scratch (required after changing the embedding model)
    Reindex(commands::index::ReindexArgs),
    /// Search the index by meaning; -word and -"a phrase" inside the query exclude results containing them
    Search(commands::search::Args),
    /// Measure search quality against golden sets of query → expected-file cases; no argument runs every
    /// set in the evals directory
    Eval(commands::eval::Args),
    /// Serve the index to AI agents over MCP (stdio)
    Mcp,
    /// Manage language plugins for indexing source code
    Lang {
        #[command(subcommand)]
        action: commands::lang::Action,
    },
    /// Manage the Claude Code skill that teaches AI agents to search with lt
    Skill {
        #[command(subcommand)]
        action: commands::skill::Action,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    commands::setup_logging(&cli.global);

    // Ctrl-C / SIGTERM drop the running command. Lance commits are atomic,
    // so an interrupted write leaves the previous version intact.
    let result = tokio::select! {
        result = run(cli) => result,
        () = shutdown_signal() => Err(anyhow::anyhow!("interrupted")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Lancet errors already carry their cause in the message;
            // `{:#}` would print it twice.
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let g = &cli.global;
    match cli.command {
        Command::Index(args) => commands::index::run(g, &args.paths, false, args.force).await,
        Command::Reindex(args) => commands::index::run(g, &args.paths, true, false).await,
        Command::Search(args) => commands::search::run(g, args).await,
        Command::Eval(args) => commands::eval::run(g, args).await,
        Command::Mcp => commands::mcp::run(g).await,
        Command::Lang { action } => commands::lang::run(g, action).await,
        Command::Skill { action } => commands::skill::run(action),
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.ok();
}
