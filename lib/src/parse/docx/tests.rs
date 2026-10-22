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
