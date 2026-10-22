use super::*;
use crate::parse::MAX_CHUNK_CHARS;

fn sections_and_texts(chunks: &[Chunk]) -> Vec<(&str, &str)> {
    chunks
        .iter()
        .map(|c| (c.section.as_str(), c.text.as_str()))
        .collect()
}

#[test]
fn heading_sections() {
    let chunks = parse_str(
        "intro text\n\n# Install\n\nfirst steps\n\n## macOS\n\nbrew stuff\n\n## Linux\n\napt stuff\n",
    );

    assert_eq!(
        sections_and_texts(&chunks),
        [
            ("", "intro text"),
            ("Install", "first steps"),
            ("Install > macOS", "brew stuff"),
            ("Install > Linux", "apt stuff"),
        ]
    );
    assert_eq!(
        chunks.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
}

#[test]
fn ignores_headings_in_code_fence() {
    let chunks = parse_str("# Title\n\n```sh\n# not a heading\necho hi\n```\n");

    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(chunks[0].section, "Title");
    assert!(
        chunks[0].text.contains("# not a heading"),
        "code fence content lost"
    );
}

#[test]
fn splits_long_section() {
    let para = "word ".repeat(100); // ~500 chars
    let chunks = parse_str(&format!("# Long\n\n{}", format!("{para}\n\n").repeat(10)));

    assert!(chunks.len() >= 2, "long section not split");
    for c in &chunks {
        assert!(c.text.len() <= MAX_CHUNK_CHARS);
        assert_eq!(c.section, "Long");
    }
}

#[test]
fn hard_splits_oversized_paragraph() {
    let chunks = parse_str(&format!("# Big\n\n{}", "а".repeat(4000))); // multi-byte on purpose

    assert!(chunks.len() >= 2, "oversized paragraph not split");
    assert!(
        chunks
            .iter()
            .all(|c| c.text.chars().count() <= MAX_CHUNK_CHARS)
    );
}

#[test]
fn empty_file() {
    assert!(parse_str("").is_empty());
}

#[test]
fn reads_a_file() {
    let (_dir, path) = crate::parse::tests::write_temp("note.md", "# T\n\nbody");
    assert_eq!(Markdown.parse(&path).unwrap().len(), 1);
}
