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
    let (_dir, store, _) = indexed_store(&[("/notes/deploy.md", "", "book the deploy slot")]).await;
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
