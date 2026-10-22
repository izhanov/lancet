use super::*;
use crate::parse::tests::write_temp;

/// A minimal but valid PDF with one page per text, xref offsets included,
/// so pdftotext parses it without repairs.
fn build_pdf(pages: &[&str]) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for (i, text) in pages.iter().enumerate() {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ));
    }

    let mut buf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.push_str(&format!("{} 0 obj\n{obj}\nendobj\n", i + 1));
    }
    let xref = buf.len();
    buf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for off in offsets {
        buf.push_str(&format!("{off:010} 00000 n \n"));
    }
    buf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    buf.into_bytes()
}

fn have_pdftotext() -> bool {
    let found = Command::new("pdftotext").arg("-v").output().is_ok();
    if !found {
        eprintln!("skipping: pdftotext not installed (brew install poppler)");
    }
    found
}

fn parse_pages(pages: &[&str]) -> Vec<Chunk> {
    let (_dir, path) = write_temp("test.pdf", build_pdf(pages));
    Pdf.parse(&path).unwrap()
}

#[test]
fn pages() {
    if !have_pdftotext() {
        return;
    }
    let chunks = parse_pages(&["alpha bravo", "charlie delta"]);

    let got: Vec<(&str, &str, usize)> = chunks
        .iter()
        .map(|c| (c.section.as_str(), c.text.as_str(), c.ordinal))
        .collect();
    assert_eq!(
        got,
        [("p. 1", "alpha bravo", 0), ("p. 2", "charlie delta", 1)]
    );
}

#[test]
fn skips_blank_pages() {
    if !have_pdftotext() {
        return;
    }
    let chunks = parse_pages(&["", "only page with text", ""]);

    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(chunks[0].section, "p. 2", "original page number kept");
}

#[test]
fn scanned_like_gives_no_chunks() {
    if !have_pdftotext() {
        return;
    }
    assert!(parse_pages(&["", ""]).is_empty());
}

#[test]
fn invalid_file() {
    if !have_pdftotext() {
        return;
    }
    let (_dir, path) = write_temp("broken.pdf", "not a pdf at all");
    assert!(Pdf.parse(&path).is_err());
}
