//! `lt mcp`: the index served to AI agents over the Model Context Protocol
//! (stdio). Read-only by design: indexing and flushing stay with the human
//! in the terminal. stdout belongs to JSON-RPC; logs go to stderr.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use lancet_lib::embed::Embedder;
use lancet_lib::search::{self, META_EMBEDDER_KEY, Mode, Options};
use lancet_lib::store::Store;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Global;

/// Upper bound on `limit`: an agent asking for more floods its own context.
const MAX_LIMIT: usize = 50;

pub async fn run(g: &Global) -> anyhow::Result<()> {
    let store = g.open_store().await?;
    let embedder = g.embedder(&store).await?;
    let server = Server::new(
        Arc::new(store),
        embedder,
        instructions(&std::path::absolute(&g.db)?),
    );
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "mcp server started");
    let service = server.serve(rmcp::transport::stdio()).await?;
    let reason = service.waiting().await?;
    tracing::info!(?reason, "mcp server stopped");
    Ok(())
}

#[derive(Clone)]
pub struct Server {
    store: Arc<Store>,
    embedder: Arc<dyn Embedder>,
    instructions: String,
    tool_router: ToolRouter<Self>,
}

/// What the agent reads about this server on connecting. Every Lancet server
/// has the same tools, so the database path is what tells them apart.
fn instructions(db: &Path) -> String {
    let db = db.display();
    let skill = if super::skill::installed() {
        format!(" Follow the {} skill.", super::skill::NAME)
    } else {
        String::new()
    };
    format!(
        "Lancet index of the database {db}: search it by meaning and by exact words before \
         reading or grepping blindly.{skill} The server is read-only: to bring the index up \
         to date, the user runs `lt --db {db} index <paths>` in a terminal."
    )
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchNotesArgs {
    /// What to look for. Full questions work well (semantic search), exact identifiers too (keyword search).
    query: String,
    /// Maximum results to return, 1-50. Default 5.
    limit: Option<usize>,
    /// hybrid (default, recommended), vector (meaning only) or text (exact words only).
    mode: Option<String>,
    /// Drop chunks containing any of these words/phrases (case-insensitive). Use for 'not about X' intents: mentioning X in the query attracts it, this filter is the only way to keep it out.
    #[serde(default)]
    exclude: Vec<String>,
    /// Drop results whose file path contains any of these fragments, e.g. a project directory name.
    #[serde(default)]
    exclude_path: Vec<String>,
}

#[derive(Serialize)]
struct IndexedFile {
    path: String,
    /// RFC 3339, UTC.
    modified: String,
}

#[tool_router]
impl Server {
    pub fn new(store: Arc<Store>, embedder: Arc<dyn Embedder>, instructions: String) -> Self {
        Self {
            store,
            embedder,
            instructions,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search the user's personal file index (notes, articles, PDFs, spreadsheets) \
            by meaning and by exact words. Returns matching chunks with the file path, section, \
            text and a relevance score (1.0 = strongest). To read more context, open the returned \
            path; ordinal is the chunk's position within that file."
    )]
    async fn search_notes(
        &self,
        Parameters(args): Parameters<SearchNotesArgs>,
    ) -> Result<String, String> {
        logged("search_notes", async {
            let opts = Options {
                mode: args.mode.as_deref().map_or(Ok(Mode::Hybrid), str::parse)?,
                limit: args.limit.unwrap_or(5).min(MAX_LIMIT),
                exclude: args.exclude,
                exclude_path: args.exclude_path,
            };
            let mut results =
                search::search(&self.store, Some(&*self.embedder), args.query.trim(), &opts)
                    .await?;
            super::search::round_scores(&mut results);
            Ok(serde_json::to_string(&results)?)
        })
        .await
    }

    #[tool(
        description = "List every indexed file with its last-modified time. Use to discover what is \
            searchable before searching, or to check whether a specific file made it into the index."
    )]
    async fn list_files(&self) -> Result<String, String> {
        logged("list_files", async {
            let mut files: Vec<IndexedFile> = self
                .store
                .paths()
                .await?
                .into_iter()
                .map(|(path, mtime)| IndexedFile {
                    path,
                    modified: rfc3339(mtime),
                })
                .collect();
            files.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(serde_json::to_string(&files)?)
        })
        .await
    }

    #[tool(
        description = "Report the state of the search index: how many chunks are stored and which \
            embedding model built it. Useful to check the index is alive before searching."
    )]
    async fn index_status(&self) -> Result<String, String> {
        logged("index_status", async {
            let chunks = self.store.count().await?;
            let meta = self.store.meta().await?;
            let embedder = meta.get(META_EMBEDDER_KEY).cloned().unwrap_or_default();
            Ok(serde_json::json!({ "chunks": chunks, "embedder": embedder }).to_string())
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("lancet", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions.clone())
    }
}

/// Runs one tool call, logging its name, duration and outcome — tool calls
/// are the events that matter. Arguments are deliberately not logged: notes
/// content may be sensitive. A failure goes back to the agent as a tool
/// error (`isError`), so it can read the message and adjust.
async fn logged<F>(tool: &str, call: F) -> Result<String, String>
where
    F: Future<Output = anyhow::Result<String>>,
{
    let start = Instant::now();
    let result = call.await;
    let took = start.elapsed();
    match &result {
        Ok(_) => tracing::info!(tool, ?took, "tool called"),
        Err(err) => tracing::error!(tool, ?took, error = %err, "tool failed"),
    }
    result.map_err(|err| err.to_string())
}

/// Unix seconds as RFC 3339 in UTC, e.g. `2026-10-22T09:30:00Z`.
fn rfc3339(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
