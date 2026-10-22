//! Embedders turn texts into vectors.

mod fake;
mod ollama;

pub use fake::Fake;
pub use ollama::Ollama;

use crate::Result;

/// Turns texts into vectors of a fixed dimension.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Identifies the vector space, e.g. "ollama/bge-m3". Vectors from
    /// different IDs are incomparable and must never share a table.
    fn id(&self) -> String;
}

/// Scales every vector to unit length in place. With unit vectors the L2
/// distance the store returns maps directly to cosine similarity, no matter
/// what the model emits.
pub fn normalize(vectors: &mut [Vec<f32>]) {
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
mod tests;
