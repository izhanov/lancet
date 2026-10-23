//! A test embedder that remembers the input type of every call.

use std::sync::Mutex;

use super::{Embedder, Fake, InputType};
use crate::Result;

/// [`Fake`] vectors, plus the input type of each `embed` call in order.
pub struct Recording {
    inner: Fake,
    inputs: Mutex<Vec<InputType>>,
}

impl Recording {
    pub fn new(dim: usize) -> Self {
        Self {
            inner: Fake { dim },
            inputs: Mutex::new(Vec::new()),
        }
    }

    pub fn inputs(&self) -> Vec<InputType> {
        self.inputs.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Embedder for Recording {
    async fn embed(&self, texts: &[String], input: InputType) -> Result<Vec<Vec<f32>>> {
        self.inputs.lock().unwrap().push(input);
        self.inner.embed(texts, input).await
    }

    fn id(&self) -> String {
        self.inner.id()
    }
}
