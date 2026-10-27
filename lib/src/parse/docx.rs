use std::fs::File;
use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;

use super::{Chunk, Chunks, Parser, io_error, parse_error, section_name};
use crate::Result;

/// Parses Word documents (.docx): a ZIP archive whose `word/document.xml`
/// holds the paragraphs. Heading styles (Heading1..Heading6) become the
/// section trail, like Markdown headings. The legacy binary .doc is not
/// supported.
pub struct Docx;

impl Parser for Docx {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        let file = File::open(path).map_err(|e| io_error(path, e))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| parse_error(path, e))?;
        let mut xml = String::new();
        match archive.by_name("word/document.xml") {
            Ok(mut entry) => entry
                .read_to_string(&mut xml)
                .map_err(|e| io_error(path, e))?,
            Err(zip::result::ZipError::FileNotFound) => return Ok(Vec::new()), // empty or malformed docx
            Err(e) => return Err(parse_error(path, e)),
        };
        let paragraphs = paragraphs(&xml).map_err(|e| parse_error(path, e))?;
        Ok(to_chunks(&paragraphs))
    }

    fn extensions(&self) -> Vec<&str> {
        vec![".docx"]
    }
}

#[derive(Default)]
struct Paragraph {
    style: String,
    text: String,
}

/// Every `w:p` in document order with its style and the text of its `w:t`
/// runs. Matching by local name also picks up paragraphs inside tables.
fn paragraphs(xml: &str) -> std::result::Result<Vec<Paragraph>, quick_xml::Error> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;

    let mut out = Vec::new();
    let mut current: Option<Paragraph> = None;
    let mut in_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(e) => match e.local_name().as_ref() {
                "p" => current = Some(Paragraph::default()),
                "t" => in_text = true,
                "pStyle" => {
                    if let Some(p) = current.as_mut()
                        && let Some(val) = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.local_name().as_ref() == "val")
                    {
                        p.style = val.value.to_string();
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                "p" => out.extend(current.take()),
                "t" => in_text = false,
                _ => {}
            },
            Event::Text(t) if in_text => {
                if let Some(p) = current.as_mut() {
                    p.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) if in_text => {
                if let Some(p) = current.as_mut() {
                    let raw = format!("&{};", &*r);
                    p.text.push_str(
                        &quick_xml::escape::unescape(&raw)
                            .map(|s| s.into_owned())
                            .unwrap_or(raw),
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn to_chunks(paragraphs: &[Paragraph]) -> Vec<Chunk> {
    let mut chunks = Chunks::default();
    let mut headings = vec![String::new(); 7]; // heading levels 1..6
    let mut body: Vec<&str> = Vec::new();
    let flush = |body: &mut Vec<&str>, headings: &[String], chunks: &mut Chunks| {
        chunks.push(&section_name(headings), body.join("\n").trim());
        body.clear();
    };

    for p in paragraphs {
        if let Some(level) = heading_level(&p.style) {
            flush(&mut body, &headings, &mut chunks);
            headings[level] = p.text.clone();
            headings[level + 1..].iter_mut().for_each(String::clear);
            continue;
        }
        if !p.text.is_empty() {
            body.push(&p.text);
        }
    }
    flush(&mut body, &headings, &mut chunks);
    chunks.chunks
}

/// "Heading1" → 1 … "Heading6" → 6.
fn heading_level(style: &str) -> Option<usize> {
    let level = style
        .strip_prefix("Heading")?
        .chars()
        .next()?
        .to_digit(10)? as usize;
    (1..=6).contains(&level).then_some(level)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A minimal DOCX: just a ZIP with word/document.xml.
    fn write_docx(document_xml: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.docx");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap();
        (dir, path)
    }

    fn document(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        )
    }

    fn heading(level: u8, text: &str) -> String {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
        )
    }

    fn para(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    #[test]
    fn headings_become_sections() {
        let (_dir, path) = write_docx(&document(
            &[
                heading(1, "Heading 1"),
                para("Body text paragraph"),
                heading(2, "Heading 2"),
                para("More body text"),
            ]
            .concat(),
        ));

        let chunks = Docx.parse(&path).unwrap();

        let got: Vec<(&str, &str)> = chunks
            .iter()
            .map(|c| (c.section.as_str(), c.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Heading 1", "Body text paragraph"),
                ("Heading 1 > Heading 2", "More body text")
            ]
        );
    }

    #[test]
    fn runs_are_joined() {
        let (_dir, path) = write_docx(&document(
            r#"<w:p><w:r><w:t>Tom </w:t></w:r><w:r><w:t xml:space="preserve">&amp; Jerry</w:t></w:r></w:p>"#,
        ));
        assert_eq!(Docx.parse(&path).unwrap()[0].text, "Tom & Jerry");
    }

    #[test]
    fn no_document_xml_means_no_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.docx");
        zip::ZipWriter::new(File::create(&path).unwrap())
            .finish()
            .unwrap();
        assert!(Docx.parse(&path).unwrap().is_empty());
    }

    #[test]
    fn not_a_zip() {
        let (_dir, path) = crate::parse::tests::write_temp("broken.docx", "not a zip");
        assert!(Docx.parse(&path).is_err());
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading_level("Heading1"), Some(1));
        assert_eq!(heading_level("Heading6"), Some(6));
        assert_eq!(heading_level("Heading7"), None);
        assert_eq!(heading_level("Title"), None);
    }
}
