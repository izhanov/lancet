//! The indexing pipeline — collect files, parse, embed, store — with
//! incremental updates by mtime. Shared by the CLI and the HTTP server,
//! which only differ in how they report progress.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};

use futures::StreamExt;
use serde::Serialize;

use crate::embed::{self, Embedder};
use crate::parse::{Chunk, Registry};
use crate::search::META_EMBEDDER_KEY;
use crate::store::{Document, Store};
use crate::{Error, Result};

pub struct Indexer {
    parsers: Registry,
    embedder: Arc<dyn Embedder>,
    /// Files processed at once.
    concurrency: usize,
}

/// Progress callback: a successfully indexed file and its chunk count.
pub type OnFile<'a> = dyn Fn(&str, usize) + Sync + 'a;

#[derive(Default)]
pub struct Options<'a> {
    /// Re-index files even when their mtime is unchanged.
    pub force: bool,
    /// Called after each file is indexed successfully, with its chunk count.
    pub on_file: Option<&'a OnFile<'a>>,
}

/// What one run did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Indexed successfully.
    pub files: usize,
    /// Could not be parsed or embedded; logged, the run went on.
    pub failed: usize,
    /// Stored by this run.
    pub chunks: usize,
    /// Unchanged, not touched.
    pub skipped: usize,
    /// Vanished from disk, dropped from the index.
    pub removed: usize,
}

impl Indexer {
    pub fn new(parsers: Registry, embedder: Arc<dyn Embedder>) -> Self {
        let concurrency = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            parsers,
            embedder,
            concurrency,
        }
    }

    /// Indexes every supported file under `roots`. Unchanged files (by
    /// mtime) are skipped, files that vanished from under the roots are
    /// dropped from the index. One broken file does not abort the run: it
    /// is logged and counted in [`Summary::failed`].
    pub async fn run(
        &self,
        store: &Store,
        roots: &[PathBuf],
        opts: &Options<'_>,
    ) -> Result<Summary> {
        let mut sum = Summary::default();
        check_embedder_meta(store, &self.embedder.id()).await?;

        let roots: Vec<PathBuf> = roots.iter().map(|r| absolute(r)).collect::<Result<_>>()?;
        let files = self.collect_files(&roots)?;
        let known = store.paths().await?;

        // Incremental indexing: an unchanged mtime means the file is skipped
        // entirely — no parsing and, above all, no embedding. Force bypasses
        // the check for files whose content changed without touching mtime.
        let mut work = Vec::new();
        for path in &files {
            if !opts.force
                && known
                    .get(&path_key(path))
                    .is_some_and(|&stored| mtime(path).ok() == Some(stored))
            {
                sum.skipped += 1;
                continue;
            }
            work.push(path.clone());
        }

        // Indexed files that vanished from the indexed directories are stale:
        // drop their chunks so search stops returning ghosts. Paths outside
        // the given roots are left alone — this run knows nothing about them.
        let on_disk: HashSet<String> = files.iter().map(|p| path_key(p)).collect();
        for stored in known.keys() {
            if on_disk.contains(stored) || !under_any_root(Path::new(stored), &roots) {
                continue;
            }
            store.delete_by_path(stored).await?;
            tracing::info!(file = %stored, "removed vanished file from index");
            sum.removed += 1;
        }

        if work.is_empty() && sum.skipped == 0 && sum.removed == 0 {
            let roots: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            return Err(Error::NoFiles(roots.join(", ")));
        }

        let mut results = futures::stream::iter(&work)
            .map(|path| async move { (path, self.index_file(store, path).await) })
            .buffer_unordered(self.concurrency);
        while let Some((path, result)) = results.next().await {
            match result {
                Ok(chunks) => {
                    sum.files += 1;
                    sum.chunks += chunks;
                    if let Some(on_file) = opts.on_file {
                        on_file(&path_key(path), chunks);
                    }
                }
                Err(err) => {
                    sum.failed += 1;
                    tracing::error!(file = %path.display(), error = %err, "failed to index");
                }
            }
        }

        if sum.files > 0 || sum.removed > 0 {
            store.optimize().await?;
        }
        Ok(sum)
    }

    /// The full pipeline for one file: parse → embed → store. Returns the
    /// number of chunks stored. The caller is expected to have checked the
    /// embedder against the store (see [`check_embedder_meta`]).
    pub async fn index_file(&self, store: &Store, path: &Path) -> Result<usize> {
        let key = path_key(path);
        let parser = self.parsers.for_path(path).ok_or_else(|| Error::Parse {
            path: key.clone(),
            message: "no parser for this file type".into(),
        })?;

        let t = Instant::now();
        let owned = path.to_path_buf();
        let chunks = tokio::task::spawn_blocking(move || parser.parse(&owned))
            .await
            .map_err(|e| Error::Parse {
                path: key.clone(),
                message: format!("parser crashed: {e}"),
            })??;
        tracing::debug!(file = %key, chunks = chunks.len(), took = ?t.elapsed(), "parsed");
        if chunks.is_empty() {
            // Nothing to store, but drop stale chunks of a now-empty file.
            store.delete_by_path(&key).await?;
            return Ok(0);
        }

        let texts: Vec<String> = chunks.iter().map(|c| embedding_text(&key, c)).collect();
        let t = Instant::now();
        let mut vectors = self
            .embedder
            .embed(&texts, embed::InputType::Document)
            .await?;
        embed::normalize(&mut vectors);
        tracing::debug!(file = %key, vectors = vectors.len(), took = ?t.elapsed(), "embedded");

        let n = chunks.len();
        let doc = Document {
            path: key.clone(),
            mtime: mtime(path)?,
            chunks,
            vectors,
        };
        let t = Instant::now();
        store.add_document(&doc).await?;
        tracing::debug!(file = %key, took = ?t.elapsed(), "stored");
        Ok(n)
    }

    /// Files under `roots` that have a registered parser. Hidden directories
    /// such as .git are skipped unless given as a root themselves. Inside a
    /// git repository `.gitignore` is honored (vendored gems, node_modules,
    /// build output), anywhere an `.ignore` file is. Overlapping roots
    /// (`notes/` and `notes/a.md`) are deduplicated, so every file is
    /// indexed at most once.
    pub fn collect_files(&self, roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for root in roots {
            // The walker reports a missing root as just another entry error.
            std::fs::metadata(root).map_err(|e| Error::Io {
                path: root.display().to_string(),
                source: e,
            })?;
            let walk = ignore::WalkBuilder::new(root)
                .hidden(false)
                .sort_by_file_name(|a, b| a.cmp(b))
                .filter_entry(|e| {
                    e.depth() == 0
                        || !(e.file_type().is_some_and(|t| t.is_dir())
                            && e.file_name().to_string_lossy().starts_with('.'))
                })
                .build();
            for entry in walk {
                // An unreadable directory or a broken ignore file costs
                // only what it covers, not the whole run.
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
                        tracing::warn!(error = %err, "skipped while collecting files");
                        continue;
                    }
                };
                let path = entry.into_path();
                if self.parsers.for_path(&path).is_some()
                    && !path.is_dir()
                    && seen.insert(path.clone())
                {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }
}

/// Refuses to mix vectors of different embedders in one database: even with
/// equal dimensions they are incomparable. A fresh database gets stamped
/// with the current embedder ID.
pub async fn check_embedder_meta(store: &Store, embedder_id: &str) -> Result<()> {
    let mut meta: HashMap<String, String> = store.meta().await?;
    match meta.get(META_EMBEDDER_KEY).map(String::as_str) {
        Some(stored) if stored == embedder_id => Ok(()),
        None | Some("") => {
            meta.insert(META_EMBEDDER_KEY.into(), embedder_id.into());
            store.set_meta(&meta).await
        }
        Some(stored) => Err(Error::EmbedderMismatch {
            stored: stored.into(),
            current: embedder_id.into(),
        }),
    }
}

/// What the embedder sees for one chunk: the telling tail of the path and
/// the heading trail woven in above the text, so a query mentioning either
/// still lands on the right vectors. The stored chunk text stays untouched —
/// this context never shows up in search output.
pub fn embedding_text(path: &str, chunk: &Chunk) -> String {
    let mut parts = vec![path_words(path)];
    if !chunk.section.is_empty() {
        parts.push(chunk.section.clone());
    }
    parts.push(chunk.text.clone());
    parts.join("\n")
}

/// The telling tail of a path as words: the file name plus up to three
/// enclosing directories, dashes and underscores split apart. The base name
/// alone is often degenerate — every agent skill is a SKILL.md whose real
/// name lives in its directory. Hidden directories and everything from the
/// home directory up are noise and are skipped.
fn path_words(path: &str) -> String {
    let mut path = Path::new(path);
    let home = std::env::home_dir();
    if let Some(rel) = home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        path = rel;
    }

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut dirs: Vec<String> = path
        .parent()
        .into_iter()
        .flat_map(|p| p.iter())
        .map(|d| d.to_string_lossy().into_owned())
        .filter(|d| d != "/" && d != "." && !d.starts_with('.'))
        .collect();
    let dirs = dirs.split_off(dirs.len().saturating_sub(3));
    let words = dirs.into_iter().chain([stem]).collect::<Vec<_>>().join(" ");
    words.replace(['-', '_'], " ")
}

/// Whether `path` lies under (or is) one of the roots.
fn under_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

/// The path as stored in the index.
fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Stored paths are absolute, so they stay valid whatever directory the
/// next command runs from.
fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|e| Error::Io {
        path: path.display().to_string(),
        source: e,
    })
}

fn mtime(path: &Path) -> Result<i64> {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| Error::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64))
}

#[cfg(test)]
mod tests;
