//! File parsers: each turns one file into chunks.

/// A fragment of a parsed file, the unit of indexing and search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Heading trail, e.g. "Install > macOS".
    pub section: String,
    pub text: String,
    /// Position within the file, keeps chunk IDs stable.
    pub ordinal: usize,
}
