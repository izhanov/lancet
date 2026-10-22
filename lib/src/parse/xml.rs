use std::path::Path;

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

use super::{Chunk, Chunks, MAX_CHUNK_CHARS, Parser, parse_error, read_to_string};
use crate::Result;

/// Indexes the text content of XML documents. Markup itself is noise for
/// embeddings, so tags never reach the chunk text — but tag names are
/// context, not garbage: the element path becomes the section
/// ("catalog > book"), leaf values are rendered as "name: value" lines,
/// attributes likewise.
pub struct Xml;

impl Parser for Xml {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        let root = parse_tree(&read_to_string(path)?).map_err(|e| parse_error(path, e))?;
        let mut chunks = Chunks::default();
        for el in &root.children {
            chunk_node(el, &el.name, &mut chunks);
        }
        Ok(chunks.chunks)
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".xml"]
    }
}

#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

/// Builds the element tree leniently: one sloppy XML file must not abort a
/// whole indexing run, so mismatched end tags just close the open element.
fn parse_tree(content: &str) -> std::result::Result<Node, quick_xml::Error> {
    let mut reader = quick_xml::Reader::from_str(content);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.expand_empty_elements = true;

    // stack[0] is a synthetic root; finished elements fold into their parent.
    let mut stack = vec![Node::default()];
    loop {
        match reader.read_event()? {
            Event::Start(e) => stack.push(start_node(&e)),
            Event::End(_) if stack.len() > 1 => {
                let node = stack.pop().expect("checked length");
                stack.last_mut().expect("root stays").children.push(node);
            }
            Event::Text(t) => stack
                .last_mut()
                .expect("root stays")
                .text
                .push_str(&t.xml10_content()),
            Event::CData(t) => stack.last_mut().expect("root stays").text.push_str(&t),
            Event::GeneralRef(r) => {
                let raw = format!("&{};", &*r);
                let resolved = quick_xml::escape::unescape(&raw)
                    .map(|s| s.into_owned())
                    .unwrap_or(raw);
                stack
                    .last_mut()
                    .expect("root stays")
                    .text
                    .push_str(&resolved);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Unclosed elements at EOF still count.
    while stack.len() > 1 {
        let node = stack.pop().expect("checked length");
        stack.last_mut().expect("root stays").children.push(node);
    }
    Ok(stack.pop().expect("root stays"))
}

fn start_node(e: &BytesStart) -> Node {
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| {
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| a.value.to_string());
            (a.key.local_name().as_ref().to_string(), value)
        })
        .collect();
    Node {
        name: e.local_name().as_ref().to_string(),
        attrs,
        ..Node::default()
    }
}

/// Walks top-down: the largest subtree that fits [`MAX_CHUNK_CHARS`]
/// becomes one chunk, so an RSS feed naturally falls apart into items and a
/// catalog into its books.
fn chunk_node(n: &Node, section: &str, chunks: &mut Chunks) {
    let rendered = render_node(n);
    if rendered.chars().count() <= MAX_CHUNK_CHARS || n.children.is_empty() {
        chunks.push(section, &rendered);
        return;
    }
    // Subtree too big: emit the node's own payload, descend into children.
    chunks.push(section, &direct_lines(n).join("\n"));
    for c in &n.children {
        chunk_node(c, &format!("{section} > {}", c.name), chunks);
    }
}

/// Flattens a subtree into "name: value" lines.
fn render_node(n: &Node) -> String {
    fn walk(c: &Node, lines: &mut Vec<String>) {
        for (key, value) in &c.attrs {
            lines.push(format!("{} {key}: {value}", c.name));
        }
        let text = c.text.trim();
        if !text.is_empty() {
            lines.push(format!("{}: {text}", c.name));
        }
        for gc in &c.children {
            walk(gc, lines);
        }
    }
    let mut lines = direct_lines(n);
    for c in &n.children {
        walk(c, &mut lines);
    }
    lines.join("\n")
}

/// Only the node's own attributes and text.
fn direct_lines(n: &Node) -> Vec<String> {
    let mut lines: Vec<String> = n.attrs.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    let text = n.text.trim();
    if !text.is_empty() {
        lines.push(text.to_string());
    }
    lines
}

#[cfg(test)]
mod tests;
