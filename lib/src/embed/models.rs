//! What each embedding model wants in front of a query and a passage.
//!
//! Asymmetric retrieval models are trained with a prompt before the text
//! (nomic-embed-text's `search_query: ` / `search_document: `); without it
//! they still return vectors, only worse ones, and nothing fails. Adding a
//! model is one row here. A model with no row is embedded as is.

use super::InputType;

/// Prompts for one model, matched by its base name: `granite-embedding:30m`
/// and `library/granite-embedding` both find `granite-embedding`.
#[derive(Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub query: &'static str,
    pub document: &'static str,
}

/// Symmetric models are listed too, with empty prompts: a row means the
/// model is known, so no warning about a missing profile.
pub const PROFILES: &[Profile] = &[
    Profile {
        name: "bge-m3",
        query: "",
        document: "",
    },
    Profile {
        name: "granite-embedding",
        query: "",
        document: "",
    },
    Profile {
        name: "all-minilm",
        query: "",
        document: "",
    },
    Profile {
        name: "paraphrase-multilingual",
        query: "",
        document: "",
    },
    Profile {
        name: "nomic-embed-text",
        query: "search_query: ",
        document: "search_document: ",
    },
    Profile {
        name: "embeddinggemma",
        query: "task: search result | query: ",
        document: "title: none | text: ",
    },
    Profile {
        name: "qwen3-embedding",
        query: "Instruct: Given a search query, retrieve relevant passages or code that answer it\nQuery:",
        document: "",
    },
    Profile {
        name: "mxbai-embed-large",
        query: "Represent this sentence for searching relevant passages: ",
        document: "",
    },
    Profile {
        name: "snowflake-arctic-embed",
        query: "Represent this sentence for searching relevant passages: ",
        document: "",
    },
    Profile {
        name: "snowflake-arctic-embed2",
        query: "query: ",
        document: "",
    },
];

/// The profile for an Ollama model name, ignoring the tag, the namespace
/// and case.
pub fn profile(model: &str) -> Option<&'static Profile> {
    let base = base_name(model);
    PROFILES.iter().find(|p| p.name.eq_ignore_ascii_case(base))
}

fn base_name(model: &str) -> &str {
    let name = model.rsplit('/').next().unwrap_or(model);
    name.split(':').next().unwrap_or(name)
}

impl Profile {
    pub fn prompt(&self, input: InputType) -> &'static str {
        match input {
            InputType::Query => self.query,
            InputType::Document => self.document,
        }
    }

    pub fn is_symmetric(&self) -> bool {
        self.query.is_empty() && self.document.is_empty()
    }

    /// Short and stable across builds, so it can go into the embedder id
    /// stored in a database: changing a prompt changes the vector space.
    pub fn fingerprint(&self) -> String {
        // FNV-1a: std's hasher is not guaranteed stable between releases.
        let mut hash: u32 = 0x811c_9dc5;
        for byte in self.query.bytes().chain([0]).chain(self.document.bytes()) {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        format!("{hash:08x}")
    }
}

#[cfg(test)]
mod tests;
