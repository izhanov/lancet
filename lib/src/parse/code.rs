//! Source code, chunked by symbol: a definition (method, class, module)
//! that fits in a chunk is one chunk, a bigger one is split into its own
//! nested definitions plus the code between them (a class header with its
//! includes and associations, say). The section is the definition trail,
//! "module Billing > class Invoice > method total", playing the part
//! headings play in Markdown. Definitions come from the language plugin's
//! tags query, see [`crate::lang`]. Code without definitions (routes,
//! specs, rake files: DSL blocks) is split along its syntax tree, with
//! block headers as the trail: "Rails.application.routes.draw > namespace
//! :account".

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::Regex;
use tree_sitter::{Node, QueryCursor, StreamingIterator, WasmStore};

use super::{Chunk, Chunks, MAX_CHUNK_CHARS, Parser, parse_error, read_to_string};
use crate::Result;
use crate::lang::{ENGINE, Language};

/// Code between definitions is packed into smaller chunks than a whole
/// definition gets: a run of DSL lines (routes, associations, config) is a
/// list of separate facts, and a vector of thirty of them blurs every one.
const PACK_CHARS: usize = 600;

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

/// A definition the tags query marks, keyed in [`Defs`] by its node's id:
/// a wrapper can span exactly the same bytes (a module body holding just
/// one class), so a byte range does not identify the node.
#[derive(Debug)]
struct Def {
    /// From the start of the line its doc comment (or itself) begins on.
    start: usize,
    /// The node's own bytes.
    node: (usize, usize),
    /// "method total", "class Billing::Invoice".
    label: String,
}

type Defs = HashMap<usize, Def>;

pub(crate) fn chunk(lang: &Language, source: &str) -> Result<Vec<Chunk>, String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_wasm_store(WasmStore::new(&ENGINE).map_err(|e| e.message)?)
        .map_err(|e| e.to_string())?;
    parser
        .set_language(&lang.grammar)
        .map_err(|e| e.to_string())?;
    let tree = parser.parse(source, None).ok_or("parser gave up")?;

    let mut walker = Walker {
        source,
        defs: definitions(lang, tree.root_node(), source),
        chunks: Chunks::default(),
    };
    walker.split(tree.root_node(), "");
    Ok(walker.chunks.chunks)
}

/// Every multi-line definition the tags query marks. One-line ones (an
/// alias, an attr) are not worth a chunk of their own and stay part of the
/// code around them.
fn definitions(lang: &Language, root: Node, source: &str) -> Defs {
    let names = lang.tags.capture_names();
    let mut defs = Defs::new();
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

        defs.entry(node.id()).or_insert(Def {
            start: line_start(source, start),
            node: range(node),
            label: format!("{kind} {name}").trim().to_string(),
        });
    }
    defs
}

/// Walks the syntax tree. Definitions go out as chunks of their own; a
/// block too big for one chunk is split along its children, its header
/// ("namespace :account", "describe Invoice") joining the trail — that is
/// what gives routes, specs and rake files, all DSL and no definitions, a
/// section. Everything else is packed in source order up to the chunk size.
struct Walker<'a> {
    source: &'a str,
    defs: Defs,
    chunks: Chunks,
}

impl Walker<'_> {
    /// Emits a node too big for one chunk, or the whole file.
    fn split(&mut self, node: Node, trail: &str) {
        let mut pack: Option<(usize, usize)> = None;
        let mut first = true;
        for item in self.items(node) {
            if let Some(def) = self.defs.get(&item.id()) {
                let (start, label) = (def.start, def.label.clone());
                self.flush(&mut pack, trail, &mut first);
                let trail = join(trail, &label);
                if item.end_byte() - start <= MAX_CHUNK_CHARS {
                    self.chunks
                        .push_code(&trail, &self.source[start..item.end_byte()]);
                } else {
                    self.split(item, &trail);
                }
                continue;
            }
            if self.is_doc(item) {
                continue; // emitted with the definition below it
            }
            let start = line_start(self.source, item.start_byte());
            if item.end_byte() - start > MAX_CHUNK_CHARS {
                self.flush(&mut pack, trail, &mut first);
                match header(self.source, item) {
                    Some(label) if item.named_child_count() > 0 => {
                        self.split(item, &join(trail, &label));
                    }
                    _ => self
                        .chunks
                        .push_code(trail, &self.source[start..item.end_byte()]),
                }
                continue;
            }
            pack = match pack {
                Some((s, _)) if item.end_byte() - s > PACK_CHARS => {
                    self.flush(&mut pack, trail, &mut first);
                    Some((start, item.end_byte()))
                }
                Some((s, _)) => Some((s, item.end_byte())),
                None => Some((start, item.end_byte())),
            };
        }
        self.flush(&mut pack, trail, &mut first);
    }

    /// The pieces `node` splits into: its named children, with wrappers
    /// that mean nothing by themselves (a body, a `do` block) opened up
    /// when they are too big or hold a definition.
    fn items<'t>(&self, node: Node<'t>) -> Vec<Node<'t>> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let wrapper = !self.defs.contains_key(&child.id())
                && header(self.source, child).is_none()
                && child.named_child_count() > 0;
            let big = child.end_byte() - child.start_byte() > MAX_CHUNK_CHARS;
            if wrapper && (big || self.holds_def(child)) {
                out.extend(self.items(child));
            } else {
                out.push(child);
            }
        }
        out
    }

    fn holds_def(&self, node: Node) -> bool {
        let (lo, hi) = range(node);
        self.defs.values().any(|d| d.node.0 >= lo && d.node.1 <= hi)
    }

    /// A comment that belongs to the definition right below it.
    fn is_doc(&self, node: Node) -> bool {
        self.defs
            .values()
            .any(|d| node.start_byte() >= d.start && node.end_byte() <= d.node.0)
    }

    fn flush(&mut self, pack: &mut Option<(usize, usize)>, trail: &str, first: &mut bool) {
        if let Some((start, end)) = pack.take() {
            let text = &self.source[start..end];
            // Inside a split block the first piece opens with the block's
            // own header line; alone ("module Billing") it only repeats
            // the trail.
            let bare_header = *first && !trail.is_empty() && meaningful_lines(text) <= 1;
            if !bare_header && meaningful_lines(text) > 0 {
                self.chunks.push_code(trail, text);
            }
        }
        *first = false;
    }
}

static OPENER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)\s*(\bdo(?:\s*\|[^|]*\|)?|\{|:|\()$").unwrap());

/// The header of a block: the first line of a multi-line node when it
/// opens one (ends in `do`, `do |x|`, `{`, `:` or `(`), without the opener —
/// "namespace :account", "describe Invoice", "if __name__ == '__main__'".
/// The opener must start a child of the node itself: a wrapper that merely
/// begins where its first statement does (a body holding a `do` block)
/// would otherwise borrow that statement's header.
fn header(source: &str, node: Node) -> Option<String> {
    let start = node.start_byte();
    let eol = source[start..]
        .find('\n')
        .map_or(source.len(), |i| start + i);
    if node.end_byte() <= eol {
        return None;
    }
    let line = source[start..eol].trim_end();
    let caps = OPENER.captures(line)?;
    let opener = start + caps.get(2)?.start();
    let mut cursor = node.walk();
    if !node.children(&mut cursor).any(|c| c.start_byte() == opener) {
        return None;
    }
    let label = caps[1].trim();
    if label.is_empty() {
        return None;
    }
    Some(match label.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", &label[..i]),
        None => label.to_string(),
    })
}

fn join(trail: &str, label: &str) -> String {
    if trail.is_empty() {
        label.to_string()
    } else {
        format!("{trail} > {label}")
    }
}

fn range(node: Node) -> (usize, usize) {
    (node.start_byte(), node.end_byte())
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
