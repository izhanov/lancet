//! Subcommands, the global flags they share and terminal output helpers.

pub mod eval;
pub mod index;
pub mod lang;
pub mod mcp;
pub mod search;
pub mod skill;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{ArgAction, ValueEnum};
use lancet_lib::config;
use lancet_lib::embed::{self, Embedder, Fake, Ollama};
use lancet_lib::parse::{Code, Registry};
use lancet_lib::search::{META_EMBEDDER_KEY, squash_whitespace};
use lancet_lib::store::{OpenOptions, Store};

/// Flags accepted by every command, each with a `LANCET_*` env twin.
#[derive(clap::Args)]
pub struct Global {
    /// Debug logging: per-file timings, retries
    #[arg(long, global = true)]
    pub verbose: bool,
    /// Errors and the final summary only
    #[arg(long, global = true)]
    pub quiet: bool,
    /// Storage backend
    #[arg(long, global = true, env = "LANCET_STORE", value_enum, default_value_t = StoreKind::Lance)]
    pub store: StoreKind,
    /// Path to the database
    #[arg(long, global = true, env = "LANCET_DB", default_value_os_t = config::default_db_path())]
    pub db: PathBuf,
    /// Embedding backend
    #[arg(long, global = true, env = "LANCET_EMBEDDER", value_enum, default_value_t = EmbedderKind::Ollama)]
    pub embedder: EmbedderKind,
    /// Ollama server URL
    #[arg(
        long,
        global = true,
        env = "LANCET_OLLAMA_URL",
        default_value = "http://localhost:11434"
    )]
    pub ollama_url: String,
    /// Ollama embedding model [default: the one the database was built
    /// with; nomic-embed-text for a new database]
    #[arg(long, global = true, env = "LANCET_EMBED_MODEL")]
    pub embed_model: Option<String>,
    /// Directory with golden-set files for `lt eval`
    #[arg(long, global = true, env = "LANCET_EVALS", default_value_os_t = config::default_evals_dir())]
    pub evals: PathBuf,
    /// Directory with language plugins for source code (`lt lang`)
    #[arg(long, global = true, env = "LANCET_LANGUAGES", default_value_os_t = config::default_languages_dir())]
    pub languages: PathBuf,
    /// Re-open the table on every read to see writes from other processes
    #[arg(long, global = true, env = "LANCET_FRESH_READS", default_value_t = true, action = ArgAction::Set)]
    pub fresh_reads: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum StoreKind {
    Lance,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum EmbedderKind {
    Ollama,
    /// Deterministic vectors without a model: for tests and dry runs
    Fake,
}

impl Global {
    pub async fn open_store(&self) -> anyhow::Result<Store> {
        let StoreKind::Lance = self.store;
        let uri = self
            .db
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("database path is not UTF-8: {}", self.db.display()))?;
        Ok(Store::open_with(
            uri,
            OpenOptions {
                fresh_reads: self.fresh_reads,
            },
        )
        .await?)
    }

    /// The built-in document parsers plus one per installed language plugin.
    pub fn parsers(&self) -> anyhow::Result<Registry> {
        let mut registry = Registry::with_defaults();
        for lang in lancet_lib::lang::load_all(&self.languages)? {
            tracing::debug!(language = %lang.manifest.name, "language plugin loaded");
            for ext in &lang.manifest.extensions {
                if registry.for_path(Path::new(&format!("x{ext}"))).is_some() {
                    tracing::warn!(
                        language = %lang.manifest.name, extension = %ext,
                        "extension already claimed by another parser; this plugin takes it over"
                    );
                }
            }
            registry.register(Code::new(lang));
        }
        Ok(registry)
    }

    /// The embedder for `store`. Without `--embed-model` a database keeps
    /// the model it was built with, so changing the default never locks an
    /// existing database out.
    pub async fn embedder(&self, store: &Store) -> anyhow::Result<Arc<dyn Embedder>> {
        Ok(match self.embedder {
            EmbedderKind::Ollama => {
                let meta = store.meta().await?;
                let stored = meta.get(META_EMBEDDER_KEY).map(String::as_str);
                let model = choose_model(self.embed_model.as_deref(), stored);
                tracing::debug!(model, "embedding model");
                Arc::new(Ollama::new(&self.ollama_url, model))
            }
            EmbedderKind::Fake => Arc::new(Fake { dim: 32 }),
        })
    }
}

/// The asked-for model, else the database's own, else the default.
fn choose_model<'a>(asked: Option<&'a str>, stored_id: Option<&'a str>) -> &'a str {
    asked
        .filter(|m| !m.is_empty())
        .or_else(|| stored_id.and_then(embed::model_from_id))
        .unwrap_or(embed::DEFAULT_MODEL)
}

/// Diagnostics go to stderr; stdout stays clean for the actual output
/// (search results, progress lines), so piping keeps working.
pub fn setup_logging(g: &Global) {
    let level = match (g.quiet, g.verbose) {
        (true, _) => "error", // --quiet wins over --verbose
        (_, true) => "debug",
        _ => "info",
    };
    // The binary crate is `lt`, its modules log as `lt::commands::...`.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| format!("warn,lancet_lib={level},lt={level}").into());
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal())
        .init();
}

pub fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

pub fn paint(enabled: bool, code: &str, s: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// A stored path made easier to read: relative to the current directory
/// when inside it, otherwise with the home directory as `~`.
pub fn pretty_path(path: &str) -> String {
    let p = Path::new(path);
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = p.strip_prefix(&cwd)
    {
        return rel.display().to_string();
    }
    if let Some(home) = std::env::home_dir()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return Path::new("~").join(rest).display().to_string();
    }
    path.to_string()
}

/// Text flattened to a single line of at most `max` characters.
pub fn snippet(text: &str, max: usize) -> String {
    let flat = squash_whitespace(text);
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_model_order() {
        let stored = Some("ollama/bge-m3");
        assert_eq!(
            choose_model(Some("mxbai-embed-large"), stored),
            "mxbai-embed-large"
        );
        assert_eq!(
            choose_model(None, stored),
            "bge-m3",
            "a database keeps its model"
        );
        assert_eq!(
            choose_model(None, Some("ollama/nomic-embed-text@30757326")),
            "nomic-embed-text"
        );
        assert_eq!(
            choose_model(None, None),
            embed::DEFAULT_MODEL,
            "a new database"
        );
        assert_eq!(
            choose_model(Some(""), None),
            embed::DEFAULT_MODEL,
            "empty env var"
        );
        assert_eq!(
            choose_model(None, Some("fake/32")),
            embed::DEFAULT_MODEL,
            "another embedder's database"
        );
    }
}
