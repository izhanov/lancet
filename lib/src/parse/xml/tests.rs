use super::*;
use crate::parse::tests::write_temp;

fn parse(content: &str) -> Vec<Chunk> {
    let (_dir, path) = write_temp("test.xml", content);
    Xml.parse(&path).unwrap()
}

#[test]
fn small_doc_is_one_chunk() {
    let chunks = parse(
        r#"<catalog>
            <book id="42">
                <title>Домик в деревне</title>
                <author>Иванов</author>
            </book>
        </catalog>"#,
    );

    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(chunks[0].section, "catalog");
    for want in ["book id: 42", "title: Домик в деревне", "author: Иванов"] {
        assert!(
            chunks[0].text.contains(want),
            "text lacks {want:?}:\n{}",
            chunks[0].text
        );
    }
    assert!(
        !chunks[0].text.contains(['<', '>']),
        "markup leaked into text"
    );
}

#[test]
fn big_doc_splits_into_subtrees() {
    let long = "слово ".repeat(200); // ~1200 chars per book
    let chunks = parse(&format!(
        "<catalog>
            <book><title>Первая</title><description>{long}</description></book>
            <book><title>Вторая</title><description>{long}</description></book>
        </catalog>"
    ));

    assert_eq!(chunks.len(), 2, "one per book");
    assert!(chunks.iter().all(|c| c.section == "catalog > book"));
    assert!(
        chunks[0].text.contains("title: Первая") && chunks[1].text.contains("title: Вторая"),
        "books mixed up"
    );
}

#[test]
fn entities_and_cdata_become_text() {
    let chunks = parse("<note><body>Tom &amp; Jerry &#x2014; <![CDATA[<raw>]]></body></note>");
    assert_eq!(chunks[0].text, "body: Tom & Jerry — <raw>");
}

#[test]
fn empty_doc() {
    assert!(parse("<root></root>").is_empty());
}

// One sloppy XML file must not abort a whole indexing run: mismatched tags
// are tolerated and the text is kept.
#[test]
fn malformed_tolerated() {
    let chunks = parse("<a><b>text</a>");
    assert!(
        chunks.len() == 1 && chunks[0].text.contains("text"),
        "{chunks:?}"
    );
}
