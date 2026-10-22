use super::*;
use crate::parse::tests::write_temp;

fn parse(name: &str, content: &str) -> Vec<Chunk> {
    let (_dir, path) = write_temp(name, content);
    Csv.parse(&path).unwrap()
}

#[test]
fn rows_with_headers() {
    let chunks = parse(
        "people.csv",
        "name,role,city\nАйгерим,engineer,Almaty\nBob,designer,Berlin\n",
    );

    assert_eq!(chunks.len(), 1, "small rows packed together: {chunks:?}");
    assert_eq!(chunks[0].section, "rows 2-3");
    for want in [
        "name: Айгерим",
        "role: engineer",
        "name: Bob",
        "city: Berlin",
    ] {
        assert!(
            chunks[0].text.contains(want),
            "text lacks {want:?}:\n{}",
            chunks[0].text
        );
    }
}

#[test]
fn packs_rows_up_to_limit() {
    let mut content = String::from("id,text\n");
    for i in 0..20 {
        content.push_str(&format!("{i},{}\n", "слово ".repeat(60))); // ~360 chars per row
    }
    let chunks = parse("big.csv", &content);

    assert!(chunks.len() >= 2, "rows must not all fit one chunk");
    for c in &chunks {
        assert!(c.text.chars().count() <= MAX_CHUNK_CHARS);
        assert!(c.section.starts_with("row"), "section = {:?}", c.section);
    }
}

#[test]
fn sniffs_semicolon_and_tab() {
    let chunks = parse("semi.csv", "name;city\nАйгерим;Almaty\n");
    assert!(
        chunks.len() == 1 && chunks[0].text.contains("city: Almaty"),
        "{chunks:?}"
    );

    let chunks = parse("tabs.tsv", "name\tcity\nБолат\tAstana\n");
    assert!(
        chunks.len() == 1 && chunks[0].text.contains("city: Astana"),
        "{chunks:?}"
    );
}

#[test]
fn ragged_row() {
    let chunks = parse("ragged.csv", "a,b\n1,2,3\n");
    assert_eq!(chunks.len(), 1);
    assert!(
        chunks[0].text.contains("col 3: 3"),
        "extra field lost its fallback name:\n{}",
        chunks[0].text
    );
}

#[test]
fn single_row_section() {
    let chunks = parse("one.csv", "a\n1\n");
    assert_eq!(chunks[0].section, "row 2");
}

#[test]
fn empty_file() {
    assert!(parse("empty.csv", "").is_empty());
}
