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
