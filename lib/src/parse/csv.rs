use std::path::Path;

use super::{Chunk, Chunks, MAX_CHUNK_CHARS, Parser, parse_error, read_to_string};
use crate::Result;

/// Indexes tabular files row by row. Column headers are woven into the text
/// ("name: Айгерим"), so every value keeps its meaning for the embedding;
/// small rows are packed together up to the chunk size limit.
pub struct Csv;

impl Parser for Csv {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        let content = read_to_string(path)?;
        let tsv = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(sniff_delimiter(&content, tsv))
            .has_headers(false)
            .flexible(true)
            .from_reader(content.as_bytes());
        let mut records = reader.records();

        let header: Vec<String> = match records.next() {
            None => return Ok(Vec::new()),
            Some(record) => record
                .map_err(|e| parse_error(path, e))?
                .iter()
                .map(str::to_string)
                .collect(),
        };

        let mut chunks = Chunks::default();
        let mut buf: Vec<String> = Vec::new(); // rendered rows of the current chunk
        let mut buf_chars = 0;
        let mut first_line = 0; // 1-based file line of the first row in buf
        let mut line = 1; // header is line 1
        let flush = |buf: &mut Vec<String>,
                     buf_chars: &mut usize,
                     first_line: usize,
                     last_line: usize,
                     chunks: &mut Chunks| {
            if buf.is_empty() {
                return;
            }
            let section = if last_line > first_line {
                format!("rows {first_line}-{last_line}")
            } else {
                format!("row {first_line}")
            };
            chunks.push(&section, &buf.join("\n\n"));
            buf.clear();
            *buf_chars = 0;
        };

        for record in records {
            let record = record.map_err(|e| parse_error(path, e))?;
            line += 1;

            let row = render_row(&header, &record);
            if row.is_empty() {
                continue;
            }
            let n = row.chars().count();
            if buf_chars > 0 && buf_chars + n + 2 > MAX_CHUNK_CHARS {
                flush(&mut buf, &mut buf_chars, first_line, line - 1, &mut chunks);
            }
            if buf.is_empty() {
                first_line = line;
            }
            buf.push(row);
            buf_chars += n + 2;
        }
        flush(&mut buf, &mut buf_chars, first_line, line, &mut chunks);
        Ok(chunks.chunks)
    }

    fn extensions(&self) -> Vec<&str> {
        vec![".csv", ".tsv"]
    }
}

/// Turns a record into self-describing "header: value" lines.
fn render_row(header: &[String], record: &csv::StringRecord) -> String {
    record
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let v = v.trim();
            if v.is_empty() {
                return None;
            }
            let name = match header.get(i).map(|h| h.trim()) {
                Some(h) if !h.is_empty() => h.to_string(),
                _ => format!("col {}", i + 1),
            };
            Some(format!("{name}: {v}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Picks the most frequent of comma, semicolon and tab on the first line;
/// the .tsv extension gives tab a head start.
fn sniff_delimiter(content: &str, tsv: bool) -> u8 {
    let first = content.lines().next().unwrap_or("");
    let mut best = if tsv { b'\t' } else { b',' };
    let mut max = 0;
    for d in *b",;\t" {
        let n = first.bytes().filter(|&b| b == d).count();
        if n > max {
            (best, max) = (d, n);
        }
    }
    best
}

#[cfg(test)]
mod tests;
