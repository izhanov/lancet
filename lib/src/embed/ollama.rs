use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Embedder, InputType};
use crate::{Error, Result};

/// Embeds texts via a local Ollama server (POST /api/embed).
/// Transient failures (network errors, 5xx) are retried with backoff;
/// client errors like an unknown model are not.
pub struct Ollama {
    base_url: String,
    model: String,
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
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
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
    /// not everything before it. `/api/embed` has no notion of [`InputType`], and
    /// bge-m3 embeds queries and passages alike, so the input type is not used.
    async fn embed(&self, texts: &[String], _input: InputType) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.batch) {
            vectors.extend(self.embed_batch(batch).await?);
        }
        Ok(vectors)
    }

    fn id(&self) -> String {
        format!("ollama/{}", self.model)
    }
}

#[cfg(test)]
mod tests;
