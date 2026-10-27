use std::hash::Hasher;

use super::{Embedder, InputType};
use crate::Result;

/// Produces deterministic pseudo-random vectors derived from the text hash.
/// Search over them is meaningless, but the vectors are valid, so the whole
/// indexing pipeline can run and be tested without a real model. The input type
/// is ignored: searching for a chunk's exact text finds that chunk.
pub struct Fake {
    pub dim: usize,
}

#[async_trait::async_trait]
impl Embedder for Fake {
    async fn embed(&self, texts: &[String], _input: InputType) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|text| self.vector(text)).collect())
    }

    fn id(&self) -> String {
        format!("fake/{}", self.dim)
    }
}

impl Fake {
    fn vector(&self, text: &str) -> Vec<f32> {
        let mut hasher = Fnv1a64::default();
        hasher.write(text.as_bytes());
        let mut state = hasher.finish();

        let mut v: Vec<f32> = (0..self.dim)
            .map(|_| {
                // xorshift64: cheap deterministic PRNG seeded by the text hash
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as i64 as f32 / i64::MAX as f32
            })
            .collect();
        super::normalize(std::slice::from_mut(&mut v));
        v
    }
}

/// FNV-1a, 64-bit: stable across runs and platforms, unlike `DefaultHasher`.
struct Fnv1a64(u64);

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv1a64 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn deterministic() {
        let f = Fake { dim: 8 };
        let a = f
            .embed(&texts(&["hello", "world"]), InputType::Document)
            .await
            .unwrap();
        let b = f
            .embed(&texts(&["hello", "world"]), InputType::Document)
            .await
            .unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn distinct_texts() {
        let v = Fake { dim: 8 }
            .embed(&texts(&["hello", "world"]), InputType::Document)
            .await
            .unwrap();
        assert_ne!(v[0], v[1], "different texts produced identical vectors");
    }

    #[tokio::test]
    async fn dimension_and_norm() {
        let v = Fake { dim: 16 }
            .embed(&texts(&["hello"]), InputType::Document)
            .await
            .unwrap();
        assert_eq!(v[0].len(), 16);
        let norm: f32 = v[0].iter().map(|x| x * x).sum();
        assert!((norm - 1.0).abs() < 1e-3, "norm = {norm}, want ~1");
    }

    #[test]
    fn id() {
        assert_eq!(Fake { dim: 32 }.id(), "fake/32");
    }
}
