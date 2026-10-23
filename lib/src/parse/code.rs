//! Source code, chunked by symbol: a definition (method, class, module)
//! that fits in a chunk is one chunk, a bigger one is split into its own
//! nested definitions plus the code between them (a class header with its
//! includes and associations, say). The section is the definition trail,
//! "module Billing > class Invoice > method total", playing the part
//! headings play in Markdown. Definitions come from the language plugin's
//! tags query, see [`crate::lang`].

use std::path::Path;
use std::sync::Arc;

use tree_sitter::{Node, QueryCursor, StreamingIterator, WasmStore};

use super::{Chunk, Chunks, MAX_CHUNK_CHARS, Parser, parse_error, read_to_string};
use crate::Result;
use crate::lang::{ENGINE, Language};

pub struct Code {
    lang: Arc<Language>,
}

impl Code {
    pub fn new(lang: Language) -> Self {
        Self {
            lang: Arc::new(lang),
        }
    }
}

impl Parser for Code {
    fn parse(&self, path: &Path) -> Result<Vec<Chunk>> {
        let source = read_to_string(path)?;
        chunk(&self.lang, &source).map_err(|message| parse_error(path, message))
    }

    fn extensions(&self) -> Vec<&str> {
        self.lang
            .manifest
            .extensions
            .iter()
            .map(String::as_str)
            .collect()
    }
}

/// A definition found by the tags query, with the definitions nested in it.
#[derive(Debug)]
struct Def {
    /// From the start of the line its doc comment (or itself) begins on.
    start: usize,
    end: usize,
    /// "method total", "class Billing::Invoice".
    label: String,
    children: Vec<Def>,
}

pub(crate) fn chunk(lang: &Language, source: &str) -> Result<Vec<Chunk>, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_wasm_store(WasmStore::new(&ENGINE).map_err(|e| e.message)?)
        .map_err(|e| e.to_string())?;
    parser
        .set_language(&lang.grammar)
        .map_err(|e| e.to_string())?;
    let tree = parser.parse(source, None).ok_or("parser gave up")?;

    let defs = nest(definitions(lang, tree.root_node(), source));
    let mut chunks = Chunks::default();
    emit_range(&mut chunks, source, 0, source.len(), &defs, "");
    Ok(chunks.chunks)
}

/// Every multi-line definition the tags query marks, sorted by position,
/// outermost first on a tie. One-line ones (an alias, an attr) are not
/// worth a chunk of their own and stay part of the code around them.
fn definitions(lang: &Language, root: Node, source: &str) -> Vec<Def> {
    let names = lang.tags.capture_names();
    let mut defs: Vec<Def> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&lang.tags, root, source.as_bytes());
    while let Some(m) = matches.next() {
        let capture = |prefix: &str| {
            m.captures()
                .iter()
                .find(|c| names[c.index as usize].starts_with(prefix))
        };
        let Some(def) = capture("definition.") else {
            continue;
        };
        let (node, kind) = (def.node, &names[def.index as usize]["definition.".len()..]);
        if node.start_position().row == node.end_position().row {
            continue;
        }
        // The node's own name field reads "Billing::Invoice" where the
        // @name capture holds only "Invoice".
        let name = node
            .child_by_field_name("name")
            .or_else(|| capture("name").map(|c| c.node))
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .unwrap_or_default();

        // Doc comments directly above, with no blank line in between.
        let mut first_row = node.start_position().row;
        let mut start = node.start_byte();
        let mut docs: Vec<Node> = m
            .captures()
            .iter()
            .filter(|c| names[c.index as usize] == "doc")
            .map(|c| c.node)
            .collect();
        docs.sort_by_key(|n| std::cmp::Reverse(n.start_byte()));
        for doc in docs {
            if doc.end_position().row + 1 != first_row {
                break;
            }
            first_row = doc.start_position().row;
            start = doc.start_byte();
        }
        let start = line_start(source, start);

        if defs
            .iter()
            .any(|d| d.start == start && d.end == node.end_byte())
        {
            continue;
        }
        defs.push(Def {
            start,
            end: node.end_byte(),
            label: format!("{kind} {name}").trim().to_string(),
            children: Vec::new(),
        });
    }
    defs.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    defs
}

/// Turns the sorted flat list into a tree by containment.
fn nest(defs: Vec<Def>) -> Vec<Def> {
    let mut roots: Vec<Def> = Vec::new();
    for def in defs {
        insert(&mut roots, def);
    }
    roots
}

fn insert(siblings: &mut Vec<Def>, def: Def) {
    match siblings.last_mut() {
        Some(last) if def.start >= last.start && def.end <= last.end => {
            insert(&mut last.children, def);
        }
        _ => siblings.push(def),
    }
}

/// Emits `source[lo..hi]`: its definitions as their own chunks, the code
/// between them packed under `trail`.
fn emit_range(chunks: &mut Chunks, source: &str, lo: usize, hi: usize, defs: &[Def], trail: &str) {
    let mut cursor = lo;
    for (i, def) in defs.iter().enumerate() {
        // Two definitions on one line: the second starts inside the first's line.
        let start = def.start.max(cursor);
        let gap = &source[cursor..start];
        // Inside a split definition the code before its first child opens
        // with the definition line itself; alone ("module Billing") it
        // only repeats the trail.
        let bare_header = i == 0 && !trail.is_empty() && meaningful_lines(gap) <= 1;
        if !bare_header {
            emit_gap(chunks, trail, gap);
        }
        emit_def(chunks, source, def, trail);
        cursor = def.end;
    }
    emit_gap(chunks, trail, &source[cursor..hi]);
}

fn emit_def(chunks: &mut Chunks, source: &str, def: &Def, parent: &str) {
    let trail = if parent.is_empty() {
        def.label.clone()
    } else {
        format!("{parent} > {}", def.label)
    };
    let text = &source[def.start..def.end];
    if text.len() <= MAX_CHUNK_CHARS || def.children.is_empty() {
        chunks.push_code(&trail, text);
    } else {
        emit_range(chunks, source, def.start, def.end, &def.children, &trail);
    }
}

/// Code between definitions. A gap of closing tokens only (`end`, `}`)
/// says nothing and is dropped.
fn emit_gap(chunks: &mut Chunks, trail: &str, text: &str) {
    if meaningful_lines(text) > 0 {
        chunks.push_code(trail, text);
    }
}

/// Lines with more to them than a closing token.
fn meaningful_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| line.chars().filter(|c| c.is_alphanumeric()).count() > 3)
        .count()
}

fn line_start(source: &str, byte: usize) -> usize {
    source[..byte].rfind('\n').map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests;
