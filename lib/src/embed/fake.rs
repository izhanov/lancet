use std::hash::Hasher;

use super::Embedder;
use crate::Result;

/// Produces deterministic pseudo-random vectors derived from the text hash.
/// Search over them is meaningless, but the vectors are valid, so the whole
/// indexing pipeline can run and be tested without a real model.
pub struct Fake {
    pub dim: usize,
}

#[async_trait::async_trait]
impl Embedder for Fake {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
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
mod tests;
