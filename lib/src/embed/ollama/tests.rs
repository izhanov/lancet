use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

/// A mock server pretending to be Ollama. Retries are near-instant so
/// tests stay fast.
async fn fake_ollama(
    respond: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
) -> (MockServer, Ollama) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(respond)
        .mount(&server)
        .await;
    let mut o = Ollama::new(&server.uri(), "bge-m3");
    o.backoff = Duration::from_millis(1);
    (server, o)
}

fn texts(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn input(req: &Request) -> Vec<String> {
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    serde_json::from_value(body["input"].clone()).unwrap()
}

#[tokio::test]
async fn embed() {
    let (server, o) = fake_ollama(|_| {
        ResponseTemplate::new(200).set_body_json(json!({"embeddings": [[0.1, 0.2], [0.3, 0.4]]}))
    })
    .await;

    let vectors = o
        .embed(&texts(&["раз", "два"]), InputType::Document)
        .await
        .unwrap();

    let body: serde_json::Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body, json!({"model": "bge-m3", "input": ["раз", "два"]}));
    assert_eq!(vectors, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);
}

#[tokio::test]
async fn client_error_not_retried() {
    let (server, o) = fake_ollama(|_| {
        ResponseTemplate::new(404).set_body_json(json!({"error": r#"model "bge-m3" not found"#}))
    })
    .await;

    let err = o
        .embed(&texts(&["x"]), InputType::Document)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("not found"),
        "server error message surfaced: {err}"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "unknown model will not appear on retry"
    );
}

#[tokio::test]
async fn retries_server_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let (_server, o) = fake_ollama(move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) < 2 {
            ResponseTemplate::new(500).set_body_json(json!({"error": "hiccup"}))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"embeddings": [[0.1]]}))
        }
    })
    .await;

    let vectors = o.embed(&texts(&["x"]), InputType::Document).await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(vectors.len(), 1);
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let (server, o) =
        fake_ollama(|_| ResponseTemplate::new(500).set_body_json(json!({"error": "still down"})))
            .await;

    let err = o
        .embed(&texts(&["x"]), InputType::Document)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("after 3 attempts"), "{err}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn count_mismatch() {
    let (_server, o) =
        fake_ollama(|_| ResponseTemplate::new(200).set_body_json(json!({"embeddings": [[0.1]]})))
            .await;

    let err = o
        .embed(&texts(&["x", "y"]), InputType::Document)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("1 embeddings for 2 texts"),
        "{err}"
    );
}

#[tokio::test]
async fn connection_refused() {
    let mut o = Ollama::new("http://127.0.0.1:1", "bge-m3"); // nothing listens there
    o.backoff = Duration::from_millis(1);

    let err = o
        .embed(&texts(&["x"]), InputType::Document)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("ollama serve"),
        "hint about ollama serve: {err}"
    );
}

#[tokio::test]
async fn batches() {
    // Echo each text's first byte back as its vector, so the test can
    // verify the batches are glued together in order.
    let (server, mut o) = fake_ollama(|req| {
        let vectors: Vec<Vec<f32>> = input(req)
            .iter()
            .map(|t| vec![f32::from(t.as_bytes()[0])])
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "embeddings": vectors }))
    })
    .await;
    o.batch = 3;

    let all: Vec<String> = ('a'..='g').map(String::from).collect();
    let vectors = o.embed(&all, InputType::Document).await.unwrap();

    let sizes: Vec<usize> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| input(r).len())
        .collect();
    assert_eq!(sizes, [3, 3, 1]);
    let firsts: Vec<f32> = vectors.iter().map(|v| v[0]).collect();
    assert_eq!(
        firsts,
        ('a'..='g').map(|c| f32::from(c as u8)).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn prompts_by_input_type() {
    let (server, mut o) = fake_ollama(|req| {
        let n = input(req).len();
        ResponseTemplate::new(200).set_body_json(json!({ "embeddings": vec![[0.1]; n] }))
    })
    .await;
    o.profile = models::profile("nomic-embed-text");

    o.embed(&texts(&["deploy"]), InputType::Query)
        .await
        .unwrap();
    o.embed(&texts(&["fn deploy()"]), InputType::Document)
        .await
        .unwrap();

    let sent: Vec<Vec<String>> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(input)
        .collect();
    assert_eq!(
        sent,
        [["search_query: deploy"], ["search_document: fn deploy()"]]
    );
}

#[test]
fn id() {
    assert_eq!(Ollama::new("http://x", "bge-m3").id(), "ollama/bge-m3");
    assert_eq!(
        Ollama::new("http://x", "hf.co/x/bge-m3-GGUF:Q8_0").id(),
        "ollama/hf.co/x/bge-m3-GGUF:Q8_0",
        "no profile: bare id"
    );
    let fingerprint = models::profile("nomic-embed-text").unwrap().fingerprint();
    assert_eq!(
        Ollama::new("http://x", "nomic-embed-text:v1.5").id(),
        format!("ollama/nomic-embed-text:v1.5@{fingerprint}")
    );
}
