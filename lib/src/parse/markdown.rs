use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Chunk, Chunks, Parser, read_to_string, section_name};
use crate::Result;

/// Chunks Markdown by headings, then by paragraphs; the heading trail
/// becomes the section. Headings inside code fences are code, not structure.
pub struct Markdown;

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*)$").unwrap());

impl Parser for Markdown {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        Ok(parse_str(&read_to_string(path)?))
    }

    fn extensions(&self) -> Vec<&str> {
        vec![".md", ".markdown"]
    }
}

fn parse_str(content: &str) -> Vec<Chunk> {
    let mut chunks = Chunks::default();
    let mut headings = vec![String::new(); 7]; // index = heading level, 1..6
    let mut body: Vec<&str> = Vec::new();
    let mut in_fence = false;

    let flush = |body: &mut Vec<&str>, headings: &[String], chunks: &mut Chunks| {
        chunks.push(&section_name(headings), body.join("\n").trim());
        body.clear();
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            body.push(line);
            continue;
        }
        if !in_fence && let Some(m) = HEADING.captures(line) {
            flush(&mut body, &headings, &mut chunks);
            let level = m[1].len();
            headings[level] = m[2].trim().to_string();
            headings[level + 1..].iter_mut().for_each(String::clear);
            continue;
        }
        body.push(line);
    }
    flush(&mut body, &headings, &mut chunks);
    chunks.chunks
}

#[cfg(test)]
mod tests {
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
}
