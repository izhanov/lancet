use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use super::{Chunk, Chunks, Parser, io_error, parse_error};
use crate::{Error, Result};

/// Extracts the text layer of a PDF via Poppler's `pdftotext`, a chunk
/// group per page. Scanned pages (no text layer) yield no chunks.
pub struct Pdf;

impl Parser for Pdf {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        // "-" sends the text to stdout; pages come separated by form feed.
        let out = Command::new("pdftotext")
            .args(["-enc", "UTF-8"])
            .arg(path)
            .arg("-")
            .output()
            .map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    Error::PdfToolMissing
                } else {
                    io_error(path, e)
                }
            })?;
        if !out.status.success() {
            return Err(parse_error(
                path,
                format!("pdftotext: {}", String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }

        let mut chunks = Chunks::default();
        for (i, page) in String::from_utf8_lossy(&out.stdout)
            .split('\x0c')
            .enumerate()
        {
            chunks.push(&format!("p. {}", i + 1), page.trim());
        }
        Ok(chunks.chunks)
    }

    fn extensions(&self) -> Vec<&str> {
        vec![".pdf"]
    }
}

#[cfg(test)]
mod tests;
