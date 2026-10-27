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

/// Prepared files are written in batches of this many files or chunks,
/// whichever fills first: one commit per batch, not per file.
const BATCH_FILES: usize = 64;
const BATCH_CHUNKS: usize = 4096;

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
            .map(|path| async move { (path, self.prepare(path).await) })
            .buffer_unordered(self.concurrency);
        let mut batch: Vec<Document> = Vec::new();
        while let Some((path, result)) = results.next().await {
            match result {
                Ok(doc) if doc.chunks.is_empty() => {
                    // Nothing to store, but drop stale chunks of a now-empty file.
                    store.delete_by_path(&doc.path).await?;
                    sum.files += 1;
                    if let Some(on_file) = opts.on_file {
                        on_file(&doc.path, 0);
                    }
                }
                Ok(doc) => {
                    batch.push(doc);
                    let chunks: usize = batch.iter().map(|d| d.chunks.len()).sum();
                    if batch.len() >= BATCH_FILES || chunks >= BATCH_CHUNKS {
                        write(store, std::mem::take(&mut batch), &mut sum, opts).await;
                    }
                }
                Err(err) => {
                    sum.failed += 1;
                    tracing::error!(file = %path.display(), error = %err, "failed to index");
                }
            }
        }
        write(store, batch, &mut sum, opts).await;

        if sum.files > 0 || sum.removed > 0 {
            store.optimize().await?;
        }
        Ok(sum)
    }

    /// The full pipeline for one file: parse → embed → store. Returns the
    /// number of chunks stored. The caller is expected to have checked the
    /// embedder against the store (see [`check_embedder_meta`]).
    pub async fn index_file(&self, store: &Store, path: &Path) -> Result<usize> {
        let doc = self.prepare(path).await?;
        if doc.chunks.is_empty() {
            // Nothing to store, but drop stale chunks of a now-empty file.
            store.delete_by_path(&doc.path).await?;
            return Ok(0);
        }
        store.add_document(&doc).await?;
        Ok(doc.chunks.len())
    }

    /// Parse → embed: one file ready to store. No chunks for an empty file.
    async fn prepare(&self, path: &Path) -> Result<Document> {
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
            return Ok(Document {
                path: key,
                mtime: mtime(path)?,
                chunks,
                vectors: Vec::new(),
            });
        }

        let texts: Vec<String> = chunks.iter().map(|c| embedding_text(&key, c)).collect();
        let t = Instant::now();
        let mut vectors = self
            .embedder
            .embed(&texts, embed::InputType::Document)
            .await?;
        embed::normalize(&mut vectors);
        tracing::debug!(file = %key, vectors = vectors.len(), took = ?t.elapsed(), "embedded");

        Ok(Document {
            mtime: mtime(path)?,
            path: key,
            chunks,
            vectors,
        })
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

/// Stores a batch of prepared files in one commit and reports them. If the
/// commit fails, the files are retried one by one, so a single bad file
/// costs only itself.
async fn write(store: &Store, batch: Vec<Document>, sum: &mut Summary, opts: &Options<'_>) {
    if batch.is_empty() {
        return;
    }
    let t = Instant::now();
    let stored: Vec<Result<()>> = match store.add_documents(&batch).await {
        Ok(()) => batch.iter().map(|_| Ok(())).collect(),
        Err(err) => {
            tracing::warn!(files = batch.len(), error = %err, "batch write failed, retrying file by file");
            let mut each = Vec::new();
            for doc in &batch {
                each.push(store.add_document(doc).await);
            }
            each
        }
    };
    tracing::debug!(files = batch.len(), took = ?t.elapsed(), "stored");
    for (doc, result) in batch.iter().zip(stored) {
        match result {
            Ok(()) => {
                sum.files += 1;
                sum.chunks += doc.chunks.len();
                if let Some(on_file) = opts.on_file {
                    on_file(&doc.path, doc.chunks.len());
                }
            }
            Err(err) => {
                sum.failed += 1;
                tracing::error!(file = %doc.path, error = %err, "failed to index");
            }
        }
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
mod tests {
    use std::fs;

    use super::*;
    use crate::embed::Fake;
    use crate::embed::recording::Recording;

    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        indexer: Indexer,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path().join("db.lance").to_str().unwrap())
                .await
                .unwrap();
            let indexer = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 8 }));
            Self {
                dir,
                store,
                indexer,
            }
        }

        /// Writes a file under the fixture's `notes/` directory.
        fn write(&self, rel: &str, content: &str) -> PathBuf {
            let path = self.notes().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }

        fn notes(&self) -> PathBuf {
            self.dir.path().join("notes")
        }

        async fn run(&self, force: bool) -> Result<Summary> {
            self.indexer
                .run(
                    &self.store,
                    &[self.notes()],
                    &Options {
                        force,
                        on_file: None,
                    },
                )
                .await
        }
    }

    #[tokio::test]
    async fn indexes_then_skips_unchanged() {
        let fx = Fixture::new().await;
        fx.write("a.md", "# A\n\none\n\n# B\n\ntwo");
        fx.write("b.csv", "name\nAigerim\n");

        let first = fx.run(false).await.unwrap();
        assert_eq!(
            first,
            Summary {
                files: 2,
                chunks: 3,
                ..Summary::default()
            }
        );
        assert_eq!(fx.store.count().await.unwrap(), 3);

        let second = fx.run(false).await.unwrap();
        assert_eq!(
            second,
            Summary {
                skipped: 2,
                ..Summary::default()
            }
        );

        let forced = fx.run(true).await.unwrap();
        assert_eq!(
            forced,
            Summary {
                files: 2,
                chunks: 3,
                ..Summary::default()
            }
        );
        assert_eq!(
            fx.store.count().await.unwrap(),
            3,
            "re-indexing does not duplicate"
        );
    }

    #[tokio::test]
    async fn stores_absolute_paths_and_mtimes() {
        let fx = Fixture::new().await;
        let path = fx.write("a.md", "text");

        fx.run(false).await.unwrap();

        let paths = fx.store.paths().await.unwrap();
        assert_eq!(paths.keys().collect::<Vec<_>>(), [&path_key(&path)]);
        assert_eq!(paths[&path_key(&path)], mtime(&path).unwrap());
    }

    #[tokio::test]
    async fn removes_vanished_files_under_the_roots_only() {
        let fx = Fixture::new().await;
        let gone = fx.write("gone.md", "bye");
        fx.write("kept.md", "hi");
        let outside = fx.dir.path().join("elsewhere.md");
        fs::write(&outside, "not under notes/").unwrap();
        fx.indexer
            .run(
                &fx.store,
                std::slice::from_ref(&outside),
                &Options::default(),
            )
            .await
            .unwrap();
        fx.run(false).await.unwrap();

        fs::remove_file(&gone).unwrap();
        let sum = fx.run(false).await.unwrap();

        assert_eq!(
            sum,
            Summary {
                skipped: 1,
                removed: 1,
                ..Summary::default()
            }
        );
        let mut paths: Vec<String> = fx.store.paths().await.unwrap().into_keys().collect();
        paths.sort();
        assert_eq!(
            paths,
            [path_key(&outside), path_key(&fx.notes().join("kept.md"))]
        );
    }

    #[tokio::test]
    async fn emptied_file_loses_its_chunks() {
        let fx = Fixture::new().await;
        let path = fx.write("a.md", "text");
        fx.run(false).await.unwrap();

        fs::write(&path, "").unwrap();
        let sum = fx.run(true).await.unwrap();

        assert_eq!(sum.chunks, 0);
        assert_eq!(fx.store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn broken_file_is_counted_not_fatal() {
        let fx = Fixture::new().await;
        fx.write("good.md", "fine");
        fx.write("bad.docx", "not a zip");

        let sum = fx.run(false).await.unwrap();

        assert_eq!(
            sum,
            Summary {
                files: 1,
                failed: 1,
                chunks: 1,
                ..Summary::default()
            }
        );
    }

    #[tokio::test]
    async fn no_files_is_an_error() {
        let fx = Fixture::new().await;
        fx.write("photo.png", "binary");

        let err = fx.run(false).await.unwrap_err();

        assert!(matches!(err, Error::NoFiles(_)), "{err}");
    }

    #[tokio::test]
    async fn refuses_a_different_embedder() {
        let fx = Fixture::new().await;
        fx.write("a.md", "text");
        fx.run(false).await.unwrap();

        let other = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 4 }));
        let err = other
            .run(&fx.store, &[fx.notes()], &Options::default())
            .await
            .unwrap_err();

        assert!(matches!(err, Error::EmbedderMismatch { .. }), "{err}");
    }

    #[tokio::test]
    async fn reports_each_file() {
        let fx = Fixture::new().await;
        let path = fx.write("a.md", &format!("one\n\n{}", "x".repeat(2000))); // 3 chunks
        let seen = std::sync::Mutex::new(Vec::new());
        let on_file = |p: &str, n: usize| seen.lock().unwrap().push((p.to_string(), n));

        fx.indexer
            .run(
                &fx.store,
                &[fx.notes()],
                &Options {
                    force: false,
                    on_file: Some(&on_file),
                },
            )
            .await
            .unwrap();

        assert_eq!(*seen.lock().unwrap(), [(path_key(&path), 3)]);
    }

    #[test]
    fn collect_files_skips_hidden_dirs_and_unknown_types() {
        let dir = tempfile::tempdir().unwrap();
        for rel in ["a.md", "sub/b.md", ".git/c.md", "photo.png"] {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let indexer = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 8 }));

        let files = indexer.collect_files(&[dir.path().to_path_buf()]).unwrap();

        assert_eq!(
            files,
            [dir.path().join("a.md"), dir.path().join("sub/b.md")]
        );
    }

    #[test]
    fn collect_files_accepts_hidden_root_and_dedups_overlaps() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(".claude/skills");
        fs::create_dir_all(skills.join("kit-pr")).unwrap();
        fs::write(skills.join("kit-pr/SKILL.md"), "x").unwrap();
        let indexer = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 8 }));

        let files = indexer
            .collect_files(&[dir.path().join(".claude"), skills.join("kit-pr/SKILL.md")])
            .unwrap();

        assert_eq!(files, [skills.join("kit-pr/SKILL.md")]);
    }

    #[test]
    fn collect_files_honors_gitignore_inside_a_repo_and_ignore_files_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, content: &str| {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        };
        write("repo/.git/HEAD", "ref: refs/heads/main\n");
        write("repo/.gitignore", "/vendor\n");
        write("repo/app.md", "x");
        write("repo/vendor/gem.md", "x");
        write("notes/.gitignore", "drafts/\n"); // not a repo: .gitignore is inert
        write("notes/.ignore", "private/\n");
        write("notes/drafts/a.md", "x");
        write("notes/private/b.md", "x");
        let indexer = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 8 }));

        let files = indexer
            .collect_files(&[dir.path().join("repo"), dir.path().join("notes")])
            .unwrap();

        assert_eq!(
            files,
            [
                dir.path().join("repo/app.md"),
                dir.path().join("notes/drafts/a.md")
            ]
        );
    }

    #[test]
    fn collect_files_missing_root_is_an_error() {
        let indexer = Indexer::new(Registry::with_defaults(), Arc::new(Fake { dim: 8 }));
        assert!(
            indexer
                .collect_files(&[PathBuf::from("/definitely/not/here")])
                .is_err()
        );
    }

    #[test]
    fn under_any_root_matches_whole_components() {
        let roots = [PathBuf::from("/notes")];
        assert!(under_any_root(Path::new("/notes/a.md"), &roots));
        assert!(under_any_root(Path::new("/notes"), &roots));
        assert!(!under_any_root(Path::new("/notes-old/a.md"), &roots));
    }

    #[test]
    fn embedding_text_weaves_in_path_and_section() {
        let chunk = |section: &str, text: &str| Chunk {
            section: section.into(),
            text: text.into(),
            ordinal: 0,
        };
        for (path, c, want) in [
            (
                "/notes/content-style-guide.md",
                chunk("Tone > Voice", "be direct"),
                "notes content style guide\nTone > Voice\nbe direct",
            ),
            (
                "/notes/llm_visibility_audit.pdf",
                chunk("", "chapter one"),
                "notes llm visibility audit\nchapter one",
            ),
            // Every agent skill is a SKILL.md; the telling name is kit-pr.
            (
                "/w/uscreen_2/.claude/skills/kit-pr/SKILL.md",
                chunk("", "open a draft PR"),
                "uscreen 2 skills kit pr SKILL\nopen a draft PR",
            ),
            (
                "docs/eval.md",
                chunk("", "golden set"),
                "docs eval\ngolden set",
            ),
        ] {
            assert_eq!(embedding_text(path, &c), want, "{path}");
        }
    }

    #[tokio::test]
    async fn embeds_chunks_as_documents() {
        let f = Fixture::new().await;
        f.write("a.md", "# A\n\nalpha\n");
        f.write("b.md", "# B\n\nbeta\n");
        let embedder = Arc::new(Recording::new(8));
        let indexer = Indexer::new(Registry::with_defaults(), embedder.clone());

        indexer
            .run(&f.store, &[f.notes()], &Options::default())
            .await
            .unwrap();

        assert_eq!(embedder.inputs(), [embed::InputType::Document; 2]);
    }
}
