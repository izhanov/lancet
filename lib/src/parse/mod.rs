//! File parsers: each turns one file into chunks.

mod csv;
mod docx;
mod markdown;
mod pdf;
mod xml;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::Regex;

pub use self::csv::Csv;
pub use self::docx::Docx;
pub use self::markdown::Markdown;
pub use self::pdf::Pdf;
pub use self::xml::Xml;
use crate::{Error, Result};

/// Keeps chunks comfortably within the embedding model context. Measured in
/// bytes when packing paragraphs, in characters when hard-splitting one, as
/// in qazyna — so Cyrillic chunks come out about half as long as Latin ones.
pub const MAX_CHUNK_CHARS: usize = 1500;

/// A fragment of a parsed file, the unit of indexing and search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Heading trail, e.g. "Install > macOS".
    pub section: String,
    pub text: String,
    /// Position within the file, keeps chunk IDs stable.
    pub ordinal: usize,
}

/// Turns one file into chunks. Parsing is blocking (file reads, the
/// `pdftotext` subprocess): call it off the async runtime.
pub trait Parser: Send + Sync {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>>;

    /// Lowercase extensions with the dot: ".md", ".markdown".
    fn extensions(&self) -> &'static [&'static str];
}

/// Maps file extensions to parsers, case-insensitively.
#[derive(Clone, Default)]
pub struct Registry {
    by_ext: HashMap<String, Arc<dyn Parser>>,
}

impl Registry {
    /// A parser for every supported format.
    pub fn with_defaults() -> Self {
        let mut registry = Self::default();
        registry.register(Markdown);
        registry.register(Pdf);
        registry.register(Xml);
        registry.register(Csv);
        registry.register(Docx);
        registry
    }

    pub fn register(&mut self, parser: impl Parser + 'static) {
        let parser: Arc<dyn Parser> = Arc::new(parser);
        for ext in parser.extensions() {
            self.by_ext.insert(ext.to_string(), parser.clone());
        }
    }

    pub fn for_path(&self, path: &Path) -> Option<Arc<dyn Parser>> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        self.by_ext.get(&format!(".{ext}")).cloned()
    }
}

/// Collects chunks with consecutive ordinals.
#[derive(Default)]
struct Chunks {
    chunks: Vec<Chunk>,
}

impl Chunks {
    /// Packs `text` into chunks under `section`; blank text adds nothing.
    fn push(&mut self, section: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        for part in split_by_paragraphs(text) {
            let ordinal = self.chunks.len();
            self.chunks.push(Chunk {
                section: section.to_string(),
                text: part,
                ordinal,
            });
        }
    }
}

/// Joins the non-empty heading levels: "Install > macOS".
fn section_name(headings: &[String]) -> String {
    headings
        .iter()
        .filter(|h| !h.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" > ")
}

static BLANK_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").unwrap());

/// Packs paragraphs into chunks of at most [`MAX_CHUNK_CHARS`]; a single
/// oversized paragraph is hard-split.
fn split_by_paragraphs(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, parts: &mut Vec<String>| {
        let s = cur.trim();
        if !s.is_empty() {
            parts.push(s.to_string());
        }
        cur.clear();
    };

    for p in BLANK_LINE.split(text) {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if p.len() > MAX_CHUNK_CHARS {
            flush(&mut cur, &mut parts);
            parts.extend(hard_split(p, MAX_CHUNK_CHARS));
            continue;
        }
        if !cur.is_empty() && cur.len() + 2 + p.len() > MAX_CHUNK_CHARS {
            flush(&mut cur, &mut parts);
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(p);
    }
    flush(&mut cur, &mut parts);
    parts
}

fn hard_split(s: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars
        .chunks(max_chars)
        .map(|c| c.iter().collect::<String>().trim().to_string())
        .collect()
}

fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| io_error(path, e))
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

fn parse_error(path: &Path, message: impl ToString) -> Error {
    Error::Parse {
        path: path.display().to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests;
