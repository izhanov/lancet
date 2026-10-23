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
mod tests;
