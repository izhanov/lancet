use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn fixture(file: &str) -> Vec<u8> {
    std::fs::read(ruby_fixture_dir().join(file)).unwrap()
}

/// A fake GitHub serving `repo` with release `tag`: its tree-sitter.json,
/// tags query and Ruby grammar from the fixture.
async fn github(repo: &str, tag: &str, config: serde_json::Value) -> (MockServer, GitHub) {
    let server = MockServer::start().await;
    let ok = |body: Vec<u8>| ResponseTemplate::new(200).set_body_bytes(body);
    Mock::given(method("GET"))
        .and(path(format!("/{repo}/releases/latest")))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("{}/{repo}/releases/tag/{tag}", server.uri()),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{repo}/releases/tag/{tag}")))
        .respond_with(ok(b"release page".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{repo}/{tag}/tree-sitter.json")))
        .respond_with(ok(config.to_string().into_bytes()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/{repo}/{tag}/queries/tags.scm")))
        .respond_with(ok(fixture(TAGS)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/{repo}/releases/download/{tag}/tree-sitter-ruby.wasm"
        )))
        .respond_with(ok(fixture(GRAMMAR)))
        .mount(&server)
        .await;
    let gh = GitHub::with_base(&server.uri(), &server.uri());
    (server, gh)
}

fn ruby_config() -> serde_json::Value {
    serde_json::json!({"grammars": [{"name": "ruby", "path": ".", "file-types": ["rb"]}]})
}

#[test]
fn loads_a_plugin() {
    let ruby = ruby_for_tests();
    assert_eq!(ruby.manifest.name, "ruby");
    assert_eq!(ruby.manifest.extensions, [".rb", ".rake", ".gemspec"]);
}

#[test]
fn fixture_grammar_matches_its_manifest() {
    let ruby = &ruby_for_tests().manifest;
    assert_eq!(
        Some(format!("{:x}", Sha256::digest(fixture(GRAMMAR)))),
        ruby.grammar_sha256
    );
}

#[tokio::test]
async fn adds_the_latest_release_falling_back_to_the_community_org() {
    let (_server, gh) = github(
        "tree-sitter-grammars/tree-sitter-ruby",
        "v9.9.9",
        ruby_config(),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();

    let added = add(dir.path(), "ruby", &[".rake".into(), "GEMSPEC".into()], &gh)
        .await
        .unwrap();

    assert_eq!(
        added,
        [Manifest {
            name: "ruby".into(),
            extensions: vec![".rb".into(), ".rake".into(), ".gemspec".into()],
            source: Some("tree-sitter-grammars/tree-sitter-ruby@v9.9.9".into()),
            grammar_sha256: ruby_for_tests().manifest.grammar_sha256.clone(),
        }]
    );
    assert_eq!(installed(dir.path()).unwrap(), added);
    assert_eq!(load_all(dir.path()).unwrap().len(), 1);
}

#[tokio::test]
async fn adds_a_pinned_tag_of_any_repository() {
    let (_server, gh) = github("someone/ruby-fork", "v1.0", ruby_config()).await;
    let dir = tempfile::tempdir().unwrap();

    let added = add(dir.path(), "someone/ruby-fork@v1.0", &[], &gh)
        .await
        .unwrap();

    assert_eq!(added[0].source.as_deref(), Some("someone/ruby-fork@v1.0"));
}

#[tokio::test]
async fn a_grammar_without_a_tags_query_is_explained() {
    let config = serde_json::json!({"grammars": [{"name": "ruby", "tags": "queries/missing.scm"}]});
    let (_server, gh) = github("o/r", "v1", config).await;
    let dir = tempfile::tempdir().unwrap();

    let err = add(dir.path(), "o/r@v1", &[], &gh)
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("has no tags query (queries/missing.scm)"),
        "{err}"
    );
    assert!(installed(dir.path()).unwrap().is_empty());
}

#[tokio::test]
async fn an_unknown_language_names_the_repositories_tried() {
    let server = MockServer::start().await;
    let gh = GitHub::with_base(&server.uri(), &server.uri());

    let err = add(Path::new("/unused"), "klingon", &[], &gh)
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("tree-sitter/tree-sitter-klingon or tree-sitter-grammars/tree-sitter-klingon"),
        "{err}"
    );
}

#[test]
fn lists_and_removes_installed_plugins() {
    let dir = tempfile::tempdir().unwrap();
    assert!(installed(dir.path()).unwrap().is_empty());
    assert!(load_all(&dir.path().join("missing")).unwrap().is_empty());
    let plugin = dir.path().join("ruby");
    std::fs::create_dir_all(&plugin).unwrap();
    for file in [MANIFEST, TAGS, GRAMMAR] {
        std::fs::copy(ruby_fixture_dir().join(file), plugin.join(file)).unwrap();
    }
    std::fs::create_dir_all(dir.path().join("not-a-plugin")).unwrap();

    let names: Vec<String> = installed(dir.path())
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["ruby"]);

    assert!(
        !remove(dir.path(), "../ruby").unwrap(),
        "a name is one path component"
    );
    assert!(remove(dir.path(), "ruby").unwrap());
    assert!(!remove(dir.path(), "ruby").unwrap());
}

#[test]
fn rejects_a_broken_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("ruby");
    std::fs::create_dir_all(&plugin).unwrap();
    for file in [MANIFEST, TAGS, GRAMMAR] {
        std::fs::copy(ruby_fixture_dir().join(file), plugin.join(file)).unwrap();
    }

    std::fs::write(plugin.join(TAGS), "(identifier) @name").unwrap();
    let err = load_all(dir.path()).err().unwrap().to_string();
    assert!(err.contains("no @definition.* captures"), "{err}");

    std::fs::copy(ruby_fixture_dir().join(TAGS), plugin.join(TAGS)).unwrap();
    std::fs::write(plugin.join(GRAMMAR), b"not wasm").unwrap();
    let err = load_all(dir.path()).err().unwrap().to_string();
    assert!(err.contains(GRAMMAR), "{err}");
}
