//! Embedders turn texts into vectors.

mod fake;
mod models;
mod ollama;
#[cfg(test)]
pub(crate) mod recording;

pub use fake::Fake;
pub use ollama::{DEFAULT_MODEL, Ollama, model_from_id};

use crate::Result;

/// What a text is embedded for. A short question and the passage that
/// answers it are not alike as texts, so retrieval models often embed the
/// two differently: Voyage's `input_type`, the `search_query:` /
/// `search_document:` prefixes of nomic-embed-text, e5's `query:` /
/// `passage:`. An embedder whose model makes no difference ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputType {
    /// A search query.
    Query,
    /// A chunk being indexed.
    Document,
}

/// Turns texts into vectors of a fixed dimension.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String], input: InputType) -> Result<Vec<Vec<f32>>>;

    /// Identifies the vector space, e.g. "ollama/bge-m3". Vectors from
    /// different IDs are incomparable and must never share a table.
    fn id(&self) -> String;
}

/// Scales every vector to unit length in place. With unit vectors the L2
/// distance the store returns maps directly to cosine similarity, no matter
/// what the model emits.
pub(crate) fn normalize(vectors: &mut [Vec<f32>]) {
    for v in vectors {
        let norm = v
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>()
            .sqrt() as f32;
        if norm == 0.0 {
            continue;
        }
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_scales_to_unit_length() {
        let mut vectors = vec![vec![3.0, 4.0], vec![0.0, 0.0]];
        normalize(&mut vectors);
        assert_eq!(vectors[0], vec![0.6, 0.8]);
        assert_eq!(vectors[1], vec![0.0, 0.0], "zero vector stays as is");
    }
}
