//! Search over a [`Store`]: semantic (vector), lexical (BM25) or hybrid,
//! fused with Reciprocal Rank Fusion. Shared by the CLI, MCP and HTTP.

use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use crate::embed::{self, Embedder};
use crate::store::{SearchResult, Store};
use crate::{Error, Result};

/// Meta key storing the [`Embedder::id`] the database was built with.
pub const META_EMBEDDER_KEY: &str = "embedder";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Hybrid,
    /// By meaning.
    Vector,
    /// By words.
    Text,
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "hybrid" => Ok(Self::Hybrid),
            "vector" => Ok(Self::Vector),
            "text" => Ok(Self::Text),
            _ => Err(Error::UnknownMode(s.to_string())),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hybrid => "hybrid",
            Self::Vector => "vector",
            Self::Text => "text",
        })
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub mode: Mode,
    /// Maximum results; 0 means the default of 5.
    pub limit: usize,
    /// Drops chunks whose text or section contains any of these terms,
    /// case-insensitive. Embeddings cannot express "not about X" —
    /// mentioning X in a query attracts it — so exclusion is a filter.
    /// The query itself may also carry exclusions: -word or -"a phrase".
    pub exclude: Vec<String>,
    /// Drops results whose file path contains any of these fragments,
    /// case-insensitive.
    pub exclude_path: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            limit: 5,
            exclude: Vec::new(),
            exclude_path: Vec::new(),
        }
    }
}

/// Runs the query against the store. The embedder may be `None` for
/// [`Mode::Text`]. Searching with a different model than the one that built
/// the index would silently return garbage, so that is refused.
pub async fn search(
    store: &Store,
    embedder: Option<&dyn Embedder>,
    query: &str,
    opts: &Options,
) -> Result<Vec<SearchResult>> {
    let limit = if opts.limit == 0 { 5 } else { opts.limit };
    let (query, mut exclude) = parse_exclusions(query);
    exclude.extend(opts.exclude.iter().cloned());
    if query.is_empty() {
        return Err(Error::EmptyQuery);
    }

    let meta = store.meta().await?;
    let stored = meta
        .get(META_EMBEDDER_KEY)
        .filter(|id| !id.is_empty())
        .ok_or(Error::EmptyDatabase)?;
    let filter = exclusion_filter(&exclude, &opts.exclude_path);
    let filter = filter.as_deref();

    if opts.mode == Mode::Text {
        return store.search_text(&query, limit, filter).await;
    }

    let embedder = embedder.ok_or(Error::NoEmbedder(opts.mode))?;
    if embedder.id() != *stored {
        return Err(Error::EmbedderMismatch {
            stored: stored.clone(),
            current: embedder.id(),
        });
    }
    let mut vectors = embedder
        .embed(std::slice::from_ref(&query), embed::InputType::Query)
        .await?;
    embed::normalize(&mut vectors);
    let vector = vectors.pop().expect("one vector per text");

    match opts.mode {
        Mode::Vector => store.search_vector(&vector, limit, filter).await,
        _ => store.search_hybrid(&vector, &query, limit, filter).await,
    }
}

static EXCLUDE_PHRASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|\s)-"([^"]+)""#).unwrap());
static EXCLUDE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)-(\S+)").unwrap());

/// Splits minus-operators out of a query: -word and -"some phrase" become
/// exclusion terms, the rest stays the query. A hyphen inside a word
/// (re-index, state-of-the-art) is not an operator.
pub fn parse_exclusions(query: &str) -> (String, Vec<String>) {
    let mut exclude = Vec::new();
    let mut strip = |re: &Regex, q: &str| {
        re.replace_all(q, |caps: &regex::Captures| {
            let term = caps[1].trim_matches('"');
            if !term.is_empty() {
                exclude.push(term.to_string());
            }
            " "
        })
        .into_owned()
    };
    let query = strip(&EXCLUDE_PHRASE, query);
    let query = strip(&EXCLUDE_WORD, &query);
    (
        query.split_whitespace().collect::<Vec<_>>().join(" "),
        exclude,
    )
}

/// An SQL predicate keeping only chunks that match no exclusion: a term in
/// the text or section, a fragment in the path. Case-insensitive substring
/// matching via `strpos`, so `%` and `_` in a term are literal.
fn exclusion_filter(terms: &[String], paths: &[String]) -> Option<String> {
    let absent = |column: &str, needle: &str| {
        format!(
            "strpos(lower(coalesce({column}, '')), '{}') = 0",
            needle.to_lowercase().replace('\'', "''")
        )
    };
    let conds: Vec<String> = terms
        .iter()
        .flat_map(|t| [absent("text", t), absent("section", t)])
        .chain(paths.iter().map(|p| absent("path", p)))
        .collect();
    (!conds.is_empty()).then(|| conds.join(" AND "))
}

#[cfg(test)]
mod tests;
