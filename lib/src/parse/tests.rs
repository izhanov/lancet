use std::path::{Path, PathBuf};

use super::*;

/// Writes `content` to `name` in a fresh temp dir; keep the dir alive while
/// the file is in use.
pub(crate) fn write_temp(name: &str, content: impl AsRef<[u8]>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    (dir, path)
}

#[test]
fn registry_matches_extensions_case_insensitively() {
    let registry = Registry::with_defaults();
    assert!(registry.for_path(Path::new("notes/readme.MD")).is_some());
    assert!(registry.for_path(Path::new("table.TSV")).is_some());
    assert!(registry.for_path(Path::new("photo.png")).is_none());
    assert!(registry.for_path(Path::new("Makefile")).is_none());
}

#[test]
fn split_packs_paragraphs_up_to_the_limit() {
    let para = "word ".repeat(100); // ~500 bytes
    let text = [para.trim(); 10].join("\n\n");

    let parts = split_by_paragraphs(&text);

    assert!(parts.len() >= 2, "long text not split");
    assert!(parts.iter().all(|p| p.len() <= MAX_CHUNK_CHARS));
    assert_eq!(
        parts[0].matches("\n\n").count(),
        1,
        "two ~500-byte paragraphs share a chunk"
    );
}

#[test]
fn hard_split_counts_characters() {
    let parts = split_by_paragraphs(&"а".repeat(4000)); // multi-byte on purpose
    assert_eq!(
        parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(),
        [1500, 1500, 1000]
    );
}

#[test]
fn section_name_skips_empty_levels() {
    let headings = ["", "Install", "", "macOS"].map(String::from);
    assert_eq!(section_name(&headings), "Install > macOS");
}
