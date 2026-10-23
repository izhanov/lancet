use super::*;

async fn open_test_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let st = Store::open(dir.path().join("test.lance").to_str().unwrap())
        .await
        .unwrap();
    (dir, st)
}

fn doc(path: &str, texts: &[&str]) -> Document {
    Document {
        path: path.to_string(),
        mtime: 1_700_000_000,
        chunks: texts
            .iter()
            .enumerate()
            .map(|(i, t)| Chunk {
                section: "s".into(),
                text: t.to_string(),
                ordinal: i,
            })
            .collect(),
        vectors: (0..texts.len())
            .map(|i| vec![i as f32, 1.0, 2.0, 3.0])
            .collect(),
    }
}

fn single(path: &str, text: &str, vector: Vec<f32>) -> Document {
    Document {
        path: path.to_string(),
        mtime: 0,
        chunks: vec![Chunk {
            section: "s".into(),
            text: text.to_string(),
            ordinal: 7,
        }],
        vectors: vec![vector],
    }
}

#[tokio::test]
async fn add_and_count() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one", "two"]))
        .await
        .unwrap();
    st.add_document(&doc("b.md", &["three"])).await.unwrap();
    assert_eq!(st.count().await.unwrap(), 3);
}

#[tokio::test]
async fn reindex_does_not_duplicate() {
    let (_dir, st) = open_test_store().await;
    for _ in 0..3 {
        st.add_document(&doc("a.md", &["one", "two"]))
            .await
            .unwrap();
    }
    assert_eq!(st.count().await.unwrap(), 2);
}

#[tokio::test]
async fn reindex_drops_chunks_gone_from_the_file() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one", "two", "three"]))
        .await
        .unwrap();
    st.add_document(&doc("b.md", &["other file"]))
        .await
        .unwrap();

    st.add_document(&doc("a.md", &["one"])).await.unwrap();

    assert_eq!(
        st.count().await.unwrap(),
        2,
        "a.md shrank to one chunk, b.md untouched"
    );
}

#[tokio::test]
async fn add_documents_replaces_many_files_in_one_commit() {
    let (_dir, st) = open_test_store().await;
    st.add_documents(&[
        doc("a.md", &["one", "two", "three"]),
        doc("b.md", &["other file"]),
        doc("c.md", &["untouched"]),
    ])
    .await
    .unwrap();
    let paths = st.paths().await.unwrap();
    assert_eq!(paths.len(), 3);

    st.add_documents(&[doc("a.md", &["one"]), doc("b.md", &["new", "text"])])
        .await
        .unwrap();

    assert_eq!(
        st.count().await.unwrap(),
        4,
        "a.md shrank to 1, b.md grew to 2, c.md kept its 1"
    );
}

#[tokio::test]
async fn delete_by_path() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one"])).await.unwrap();
    st.add_document(&doc("b.md", &["two"])).await.unwrap();
    st.delete_by_path("a.md").await.unwrap();
    assert_eq!(st.count().await.unwrap(), 1);
}

#[tokio::test]
async fn empty_store() {
    let (_dir, st) = open_test_store().await;
    // No table exists yet: everything must work, not fail.
    st.delete_by_path("a.md").await.unwrap();
    assert_eq!(st.count().await.unwrap(), 0);
    assert!(st.paths().await.unwrap().is_empty());
    assert!(
        st.search_vector(&[1.0, 0.0], 5, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        st.search_text("anything", 5, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        st.search_hybrid(&[1.0, 0.0], "anything", 5, None)
            .await
            .unwrap()
            .is_empty()
    );
    st.optimize().await.unwrap();
}

// The MCP-server scenario: a long-lived reader opened before another
// process wrote must still see the new data.
#[tokio::test]
async fn fresh_reads_see_other_writers() {
    let dir = tempfile::tempdir().unwrap();
    let uri = dir.path().join("test.lance");
    let uri = uri.to_str().unwrap();

    let reader = Store::open(uri).await.unwrap();
    let writer = Store::open(uri).await.unwrap();
    writer
        .add_document(&doc("a.md", &["written after reader opened"]))
        .await
        .unwrap();

    assert_eq!(reader.count().await.unwrap(), 1, "stale table handle?");
    assert_eq!(
        reader.search_text("written", 5, None).await.unwrap().len(),
        1
    );
}

// With fresh reads off the handle stays pinned: concurrent writers are
// invisible until reopen.
#[tokio::test]
async fn pinned_reads_ignore_other_writers() {
    let dir = tempfile::tempdir().unwrap();
    let uri = dir.path().join("test.lance");
    let uri = uri.to_str().unwrap();

    // The table must exist before the reader opens, otherwise there is
    // nothing to pin.
    let writer = Store::open(uri).await.unwrap();
    writer.add_document(&doc("a.md", &["first"])).await.unwrap();
    let reader = Store::open_with(uri, OpenOptions { fresh_reads: false })
        .await
        .unwrap();
    writer
        .add_document(&doc("b.md", &["second"]))
        .await
        .unwrap();

    assert_eq!(
        reader.count().await.unwrap(),
        1,
        "pinned handle must not see b.md"
    );
}

#[tokio::test]
async fn vector_search_scores_are_cosine() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&single("a.md", "alpha", vec![1.0, 0.0, 0.0, 0.0]))
        .await
        .unwrap();
    st.add_document(&single("b.md", "beta", vec![0.0, 1.0, 0.0, 0.0]))
        .await
        .unwrap(); // orthogonal

    let results = st
        .search_vector(&[1.0, 0.0, 0.0, 0.0], 2, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    let best = &results[0];
    assert_eq!(
        (
            best.path.as_str(),
            best.text.as_str(),
            best.section.as_str(),
            best.ordinal
        ),
        ("a.md", "alpha", "s", 7)
    );
    // Identical unit vectors → cosine 1; orthogonal → 0. This pins down
    // the _distance → cosine conversion, not just the ordering.
    assert!(
        (best.score - 1.0).abs() < 0.01,
        "best score = {}",
        best.score
    );
    assert!(
        results[1].score.abs() < 0.05,
        "orthogonal score = {}",
        results[1].score
    );
}

#[tokio::test]
async fn text_search_is_case_insensitive() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc(
        "a.md",
        &["Doorkeeper отзывает его лениво", "просто текст про хлеб"],
    ))
    .await
    .unwrap();

    let results = st.search_text("Лениво", 5, None).await.unwrap();

    assert_eq!(
        results.len(),
        1,
        "exactly the chunk with the word: {results:?}"
    );
    assert!(results[0].text.contains("лениво"));
    assert_eq!(results[0].score, 1.0, "top hit scales to 1");
}

#[tokio::test]
async fn text_search_matches_whole_words() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc(
        "a.md",
        &[
            "prepare the approach for printing",
            "push the branch and open draft PRs",
        ], // "pr" only as a substring / plural
    ))
    .await
    .unwrap();

    let results = st.search_text("PRs", 5, None).await.unwrap();

    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].text.contains("PRs"));
}

#[tokio::test]
async fn text_search_sees_rows_added_after_the_index() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["first file"]))
        .await
        .unwrap(); // builds the FTS index
    st.add_document(&doc("b.md", &["second newcomer"]))
        .await
        .unwrap(); // not in the index yet

    assert_eq!(st.search_text("newcomer", 5, None).await.unwrap().len(), 1);
    st.optimize().await.unwrap();
    assert_eq!(st.search_text("newcomer", 5, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn hybrid_rewards_agreement() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&single(
        "a.md",
        "deploy to preprod",
        vec![1.0, 0.0, 0.0, 0.0],
    ))
    .await
    .unwrap();
    st.add_document(&single("b.md", "lunch menu", vec![0.9, 0.1, 0.0, 0.0]))
        .await
        .unwrap();
    // Longer text, lower BM25: a.md is unambiguously #1 by text too.
    st.add_document(&single(
        "c.md",
        "preprod lunch for the whole team today",
        vec![0.0, 1.0, 0.0, 0.0],
    ))
    .await
    .unwrap();
    // BM25 of indexed and not-yet-indexed rows is computed from different
    // statistics; the indexer optimizes after every run, so do it here.
    st.optimize().await.unwrap();

    let results = st
        .search_hybrid(&[1.0, 0.0, 0.0, 0.0], "preprod", 2, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2, "limit applies after fusion");
    assert_eq!(results[0].path, "a.md", "#1 by vector and by text");
    assert!(
        (results[0].score - 1.0).abs() < 1e-6,
        "both-#1 score = {}",
        results[0].score
    );
}

#[tokio::test]
async fn filter_applies_before_search() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&single("a.md", "deploy notes", vec![1.0, 0.0, 0.0, 0.0]))
        .await
        .unwrap();
    st.add_document(&single(
        "b.md",
        "deploy notes too",
        vec![0.0, 1.0, 0.0, 0.0],
    ))
    .await
    .unwrap();

    let filter = Some("path != 'a.md'");
    for results in [
        st.search_vector(&[1.0, 0.0, 0.0, 0.0], 1, filter)
            .await
            .unwrap(),
        st.search_text("deploy", 1, filter).await.unwrap(),
        st.search_hybrid(&[1.0, 0.0, 0.0, 0.0], "deploy", 1, filter)
            .await
            .unwrap(),
    ] {
        assert_eq!(results.len(), 1, "filtered rows must not eat the limit");
        assert_eq!(results[0].path, "b.md");
    }
}

#[tokio::test]
async fn paths() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one", "two"]))
        .await
        .unwrap();
    st.add_document(&doc("b.md", &["three"])).await.unwrap();

    let paths = st.paths().await.unwrap();

    assert_eq!(
        paths,
        HashMap::from([
            ("a.md".into(), 1_700_000_000),
            ("b.md".into(), 1_700_000_000)
        ])
    );
}

#[tokio::test]
async fn meta_roundtrip_replaces_the_whole_map() {
    let (_dir, st) = open_test_store().await;
    assert!(st.meta().await.unwrap().is_empty());

    st.set_meta(&HashMap::from([
        ("embedder".into(), "ollama/bge-m3".into()),
        ("old".into(), "x".into()),
    ]))
    .await
    .unwrap();
    st.set_meta(&HashMap::from([("embedder".into(), "fake/32".into())]))
        .await
        .unwrap();

    assert_eq!(
        st.meta().await.unwrap(),
        HashMap::from([("embedder".into(), "fake/32".into())])
    );
}

#[tokio::test]
async fn reset_allows_new_dimension() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one"])).await.unwrap(); // dim 4
    st.set_meta(&HashMap::from([("embedder".into(), "fake/4".into())]))
        .await
        .unwrap();

    st.reset().await.unwrap();

    assert_eq!(st.count().await.unwrap(), 0);
    assert!(st.meta().await.unwrap().is_empty());
    st.add_document(&single("b.md", "x", vec![1.0, 2.0]))
        .await
        .unwrap(); // dim 2 now accepted
}

#[tokio::test]
async fn vector_dimension_mismatch() {
    let (_dir, st) = open_test_store().await;
    st.add_document(&doc("a.md", &["one"])).await.unwrap();

    let err = st
        .add_document(&single("b.md", "x", vec![1.0, 2.0]))
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::DimensionMismatch { table: 4, got: 2 }),
        "{err}"
    );
}
