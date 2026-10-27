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
fn parse_exclusions(query: &str) -> (String, Vec<String>) {
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
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::embed::recording::Recording;
    use crate::embed::{Fake, InputType};
    use crate::parse::Chunk;
    use crate::store::Document;

    #[test]
    fn mode_from_str() {
        assert_eq!("text".parse::<Mode>().unwrap(), Mode::Text);
        assert!(matches!(
            "telepathy".parse::<Mode>(),
            Err(Error::UnknownMode(_))
        ));
    }

    #[test]
    fn parse_exclusions_cases() {
        for (query, positive, exclude) in [
            (
                r#"improve search -"evil martians" -preprod"#,
                "improve search",
                vec!["evil martians", "preprod"],
            ),
            ("-slack how to deploy", "how to deploy", vec!["slack"]),
            (
                "re-index state-of-the-art notes",
                "re-index state-of-the-art notes",
                vec![],
            ),
            ("deploy - ticket", "deploy - ticket", vec![]), // bare minus is not an operator
            (r#"-"only exclusions""#, "", vec!["only exclusions"]),
        ] {
            let (got_positive, got_exclude) = parse_exclusions(query);
            assert_eq!(got_positive, positive, "positive part of {query:?}");
            assert_eq!(got_exclude, exclude, "exclusions of {query:?}");
        }
    }

    /// A store indexed with the fake embedder: searching for a chunk's exact
    /// text yields that chunk's vector, so vector search is predictable.
    async fn indexed_store(files: &[(&str, &str, &str)]) -> (tempfile::TempDir, Store, Fake) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("db.lance").to_str().unwrap())
            .await
            .unwrap();
        let fake = Fake { dim: 8 };
        for (path, section, text) in files {
            let vectors = fake
                .embed(&[text.to_string()], InputType::Document)
                .await
                .unwrap();
            let chunks = vec![Chunk {
                section: section.to_string(),
                text: text.to_string(),
                ordinal: 0,
            }];
            store
                .add_document(&Document {
                    path: path.to_string(),
                    mtime: 0,
                    chunks,
                    vectors,
                })
                .await
                .unwrap();
        }
        store
            .set_meta(&HashMap::from([(META_EMBEDDER_KEY.into(), fake.id())]))
            .await
            .unwrap();
        (dir, store, fake)
    }

    fn paths(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.path.as_str()).collect()
    }

    #[tokio::test]
    async fn every_mode_finds_the_chunk() {
        let (_dir, store, fake) = indexed_store(&[
            ("/notes/deploy.md", "", "book the deploy slot"),
            ("/notes/lunch.md", "", "lunch menu"),
        ])
        .await;

        for mode in [Mode::Vector, Mode::Text, Mode::Hybrid] {
            let opts = Options {
                mode,
                limit: 1,
                ..Options::default()
            };
            let results = search(&store, Some(&fake), "book the deploy slot", &opts)
                .await
                .unwrap();
            assert_eq!(paths(&results), ["/notes/deploy.md"], "mode {mode}");
        }
    }

    #[tokio::test]
    async fn exclusions_filter_text_section_and_path() {
        let (_dir, store, fake) = indexed_store(&[
            ("/home/notes/deploy.md", "", "book the deploy slot"),
            (
                "/home/notes/style.md",
                "Evil Martians Style Guide",
                "voice rules for the deploy",
            ),
            (
                "/home/evilmartians/uscreen/skill.md",
                "",
                "run the deploy trigger",
            ),
        ])
        .await;
        let run = |query: &'static str, exclude_path: Vec<String>| {
            let (store, fake) = (&store, &fake);
            async move {
                let opts = Options {
                    mode: Mode::Text,
                    limit: 10,
                    exclude: vec![],
                    exclude_path,
                };
                let mut found = paths(&search(store, Some(fake), query, &opts).await.unwrap())
                    .into_iter()
                    .map(String::from)
                    .collect::<Vec<_>>();
                found.sort();
                found
            }
        };

        assert_eq!(
            run(r#"deploy -"EVIL MARTIANS""#, vec![]).await,
            [
                "/home/evilmartians/uscreen/skill.md",
                "/home/notes/deploy.md"
            ],
            "term matched in the section"
        );
        assert_eq!(
            run("deploy", vec!["EvilMartians".into()]).await,
            ["/home/notes/deploy.md", "/home/notes/style.md"]
        );
        assert_eq!(
            run("deploy -slot", vec!["evilmartians".into()]).await,
            ["/home/notes/style.md"]
        );
    }

    #[tokio::test]
    async fn exclusion_terms_are_literal() {
        let (_dir, store, fake) = indexed_store(&[
            ("/a.md", "", "set dry_run for the deploy"),
            ("/b.md", "", "dryXrun is not it, deploy"),
        ])
        .await;

        for term in ["dry_run", "100%", "it's"] {
            let opts = Options {
                mode: Mode::Text,
                limit: 10,
                exclude: vec![term.into()],
                ..Options::default()
            };
            let results = search(&store, Some(&fake), "deploy", &opts).await.unwrap();
            if term == "dry_run" {
                assert_eq!(paths(&results), ["/b.md"], "_ must not match any character");
            } else {
                assert_eq!(results.len(), 2, "{term:?} excludes nothing");
            }
        }
    }

    #[tokio::test]
    async fn refuses_bad_requests() {
        let (_dir, store, fake) = indexed_store(&[("/a.md", "", "text")]).await;
        let hybrid = Options::default();

        let err = search(&store, Some(&fake), r#"-"evil martians""#, &hybrid)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::EmptyQuery), "{err}");

        let err = search(&store, None, "text", &hybrid).await.unwrap_err();
        assert!(matches!(err, Error::NoEmbedder(Mode::Hybrid)), "{err}");

        let err = search(&store, Some(&Fake { dim: 4 }), "text", &hybrid)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::EmbedderMismatch { .. }), "{err}");

        let text = Options {
            mode: Mode::Text,
            ..Options::default()
        };
        assert_eq!(
            search(&store, None, "text", &text).await.unwrap().len(),
            1,
            "text mode needs no embedder"
        );
    }

    #[tokio::test]
    async fn refuses_an_empty_database() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("db.lance").to_str().unwrap())
            .await
            .unwrap();

        let err = search(&store, None, "anything", &Options::default())
            .await
            .unwrap_err();

        assert!(matches!(err, Error::EmptyDatabase), "{err}");
    }

    #[tokio::test]
    async fn embeds_the_query_as_a_query() {
        let (_dir, store, _) =
            indexed_store(&[("/notes/deploy.md", "", "book the deploy slot")]).await;
        let embedder = Recording::new(8);

        for mode in [Mode::Vector, Mode::Hybrid, Mode::Text] {
            let opts = Options {
                mode,
                ..Options::default()
            };
            search(&store, Some(&embedder), "deploy", &opts)
                .await
                .unwrap();
        }

        // Text mode needs no vector.
        assert_eq!(embedder.inputs(), [InputType::Query; 2]);
    }
}
