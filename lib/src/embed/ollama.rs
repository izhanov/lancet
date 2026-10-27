use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::models::{self, Profile};
use super::{Embedder, InputType};
use crate::{Error, Result};

/// Embeds texts via a local Ollama server (POST /api/embed).
/// Transient failures (network errors, 5xx) are retried with backoff;
/// client errors like an unknown model are not.
pub struct Ollama {
    base_url: String,
    model: String,
    profile: Option<&'static Profile>, // prompts before queries and passages
    client: reqwest::Client,

    timeout: Duration, // per-request deadline
    attempts: u32,
    backoff: Duration, // first retry delay, doubles per attempt
    batch: usize,      // max texts per request; Ollama's runner dies on huge inputs
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
    #[serde(default)]
    error: String,
}

impl Ollama {
    /// The generous per-request timeout covers the first call loading the
    /// model into memory.
    pub fn new(base_url: &str, model: &str) -> Self {
        let profile = models::profile(model);
        if profile.is_none() {
            tracing::warn!(
                model,
                "no prompt profile for this model, queries and passages are embedded as is"
            );
        }
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            profile,
            client: reqwest::Client::new(),
            timeout: Duration::from_secs(5 * 60),
            attempts: 3,
            backoff: Duration::from_secs(1),
            batch: 32,
        }
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut last = None;
        for attempt in 0..self.attempts {
            if attempt > 0 {
                tokio::time::sleep(self.backoff * 2u32.pow(attempt - 1)).await;
            }
            match self.embed_once(texts).await {
                Ok(vectors) => return Ok(vectors),
                Err(err) if retryable(&err) => {
                    tracing::warn!(attempt = attempt + 1, of = self.attempts, error = %err, "ollama request failed, will retry");
                    last = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(Error::OllamaGaveUp {
            attempts: self.attempts,
            last: Box::new(last.expect("at least one attempt")),
        })
    }

    async fn embed_once(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.base_url);
        let unreachable = |source| Error::OllamaUnreachable {
            url: self.base_url.clone(),
            source,
        };

        let resp = self
            .client
            .post(&url)
            .timeout(self.timeout)
            .json(&EmbedRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(unreachable)?;
        let status = resp.status().as_u16();
        let raw = resp.bytes().await.map_err(unreachable)?;

        let parsed: EmbedResponse = serde_json::from_slice(&raw).map_err(|_| Error::Ollama {
            status,
            message: format!(
                "unexpected response: {:.200}",
                String::from_utf8_lossy(&raw)
            ),
        })?;
        if status != 200 || !parsed.error.is_empty() {
            return Err(Error::Ollama {
                status,
                message: parsed.error,
            });
        }
        if parsed.embeddings.len() != texts.len() {
            return Err(Error::OllamaResponse(format!(
                "returned {} embeddings for {} texts",
                parsed.embeddings.len(),
                texts.len()
            )));
        }
        if let Some(i) = parsed.embeddings.iter().position(Vec::is_empty) {
            return Err(Error::OllamaResponse(format!(
                "returned an empty embedding for text {i}"
            )));
        }
        Ok(parsed.embeddings)
    }
}

/// Network errors and timeouts are worth retrying, and so is a 5xx: the
/// server hiccuping. A 4xx (unknown model etc.) will not get better.
fn retryable(err: &Error) -> bool {
    match err {
        Error::OllamaUnreachable { .. } => true,
        Error::Ollama { status, .. } => *status >= 500,
        _ => false,
    }
}

#[async_trait::async_trait]
impl Embedder for Ollama {
    /// Slices texts into batches: a whole book in one request kills Ollama's
    /// runner, and a failed retry then re-sends only the batch that broke,
    /// not everything before it. `/api/embed` has no notion of [`InputType`],
    /// so the model's prompt for it goes in front of each text.
    async fn embed(&self, texts: &[String], input: InputType) -> Result<Vec<Vec<f32>>> {
        let prompt = self.profile.map_or("", |p| p.prompt(input));
        let prompted: Vec<String>;
        let texts = if prompt.is_empty() {
            texts
        } else {
            prompted = texts.iter().map(|t| format!("{prompt}{t}")).collect();
            &prompted
        };

        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.batch) {
            vectors.extend(self.embed_batch(batch).await?);
        }
        Ok(vectors)
    }

    /// Prompts shape the vectors, so they are part of the id; models
    /// without them keep the bare id their databases were stamped with.
    fn id(&self) -> String {
        match self.profile {
            Some(p) if !p.is_symmetric() => format!("ollama/{}@{}", self.model, p.fingerprint()),
            _ => format!("ollama/{}", self.model),
        }
    }
}

#[cfg(test)]
mod tests {
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
            ResponseTemplate::new(200)
                .set_body_json(json!({"embeddings": [[0.1, 0.2], [0.3, 0.4]]}))
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
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": r#"model "bge-m3" not found"#}))
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
        let (server, o) = fake_ollama(|_| {
            ResponseTemplate::new(500).set_body_json(json!({"error": "still down"}))
        })
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
        let (_server, o) = fake_ollama(|_| {
            ResponseTemplate::new(200).set_body_json(json!({"embeddings": [[0.1]]}))
        })
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
}
