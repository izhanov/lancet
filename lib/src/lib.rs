//! Lancet core: parsing, embedding, indexing and hybrid search over LanceDB.

pub mod embed;
pub mod parse;
pub mod search;
pub mod store;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Lance(#[from] lancedb::Error),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("unexpected table layout: {0}")]
    Schema(String),


    #[error("ollama at {url}: {source} (is `ollama serve` running?)")]
    OllamaUnreachable { url: String, source: reqwest::Error },
    #[error("ollama (HTTP {status}): {message}")]
    Ollama { status: u16, message: String },
    #[error("ollama: {0}")]
    OllamaResponse(String),
    #[error("after {attempts} attempts: {last}")]
    OllamaGaveUp { attempts: u32, last: Box<Error> },

    #[error("chunks/vectors mismatch: {chunks} vs {vectors}")]
    ChunksVectorsMismatch { chunks: usize, vectors: usize },
    #[error("vector dimension mismatch: table has {table}, got {got}")]
    DimensionMismatch { table: usize, got: usize },
    #[error("database is indexed with embedder {stored:?}, current is {current:?}")]
    EmbedderMismatch { stored: String, current: String },
    #[error("database is empty; run `lt index <path>` first")]
    EmptyDatabase,

    #[error("unknown mode {0:?} (want hybrid, vector or text)")]
    UnknownMode(String),
    #[error("mode {0} needs an embedder")]
    NoEmbedder(search::Mode),
    #[error("query is empty after removing exclusions")]
    EmptyQuery,
}
