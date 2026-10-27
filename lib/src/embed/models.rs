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
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn matches_base_name() {
        for name in [
            "nomic-embed-text",
            "nomic-embed-text:latest",
            "nomic-embed-text:v1.5",
            "library/nomic-embed-text",
            "Nomic-Embed-Text",
        ] {
            assert_eq!(
                profile(name).map(|p| p.name),
                Some("nomic-embed-text"),
                "{name}"
            );
        }
    }

    #[test]
    fn tag_does_not_pick_another_model() {
        assert_eq!(
            profile("snowflake-arctic-embed:s").unwrap().name,
            "snowflake-arctic-embed"
        );
        assert_eq!(
            profile("snowflake-arctic-embed2:568m").unwrap().name,
            "snowflake-arctic-embed2"
        );
    }

    #[test]
    fn unknown_model() {
        assert_eq!(profile("hf.co/gpustack/bge-m3-GGUF:Q8_0"), None);
        assert_eq!(profile(""), None);
    }

    #[test]
    fn prompt_by_input_type() {
        let p = profile("nomic-embed-text").unwrap();
        assert_eq!(p.prompt(InputType::Query), "search_query: ");
        assert_eq!(p.prompt(InputType::Document), "search_document: ");
        assert!(!p.is_symmetric());
        assert!(profile("bge-m3").unwrap().is_symmetric());
    }

    #[test]
    fn names_are_unique() {
        let mut seen = HashSet::new();
        for p in PROFILES {
            assert!(seen.insert(p.name), "{} listed twice", p.name);
            assert_eq!(base_name(p.name), p.name, "{} must be a base name", p.name);
        }
    }

    #[test]
    fn fingerprint_is_stable() {
        // A changed value here means every database built with this profile
        // stops opening: change it only together with the prompt.
        assert_eq!(
            profile("nomic-embed-text").unwrap().fingerprint(),
            "30757326"
        );
    }

    #[test]
    fn fingerprint_tells_query_from_document() {
        let a = Profile {
            name: "a",
            query: "x",
            document: "",
        };
        let b = Profile {
            name: "a",
            query: "",
            document: "x",
        };
        assert_ne!(a.fingerprint(), b.fingerprint());
    }
}
