use super::*;
use crate::lang::ruby_for_tests;

fn chunks(source: &str) -> Vec<(String, String)> {
    chunk(ruby_for_tests(), source)
        .unwrap()
        .into_iter()
        .map(|c| (c.section, c.text))
        .collect()
}

#[test]
fn a_small_class_is_one_chunk() {
    let source =
        "require \"json\"\n\nclass Invoice\n  def total\n    items.sum(&:price)\n  end\nend\n";

    assert_eq!(
        chunks(source),
        [
            ("".into(), "require \"json\"".into()),
            (
                "class Invoice".into(),
                "class Invoice\n  def total\n    items.sum(&:price)\n  end\nend".into()
            ),
        ]
    );
}

#[test]
fn a_big_class_splits_into_header_and_methods() {
    let filler = "    # keeps the class above the chunk size\n".repeat(20);
    let source = format!(
        "module Billing\n  class Invoice < ApplicationRecord\n    belongs_to :store\n\n    \
         # Sum of the line items, in cents.\n    def total\n{filler}      items.sum(&:price)\n    end\n\n    \
         alias_method :amount, :total\n\n    def self.overdue\n{filler}      where(paid: false)\n    end\n  end\nend\n"
    );

    let got = chunks(&source);

    let sections: Vec<&str> = got.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        sections,
        [
            "module Billing > class Invoice",
            "module Billing > class Invoice > method total",
            "module Billing > class Invoice",
            "module Billing > class Invoice > method overdue",
        ]
    );
    assert_eq!(
        got[0].1,
        "  class Invoice < ApplicationRecord\n    belongs_to :store"
    );
    assert!(
        got[1]
            .1
            .starts_with("    # Sum of the line items, in cents.\n    def total\n"),
        "the doc comment belongs to its method: {:?}",
        got[1].1
    );
    // A one-line definition stays with the code around it.
    assert_eq!(got[2].1, "    alias_method :amount, :total");
}

#[test]
fn a_qualified_name_is_kept_whole() {
    let got = chunks("class Billing::Invoice\n  def total; end\nend\n");
    assert_eq!(got[0].0, "class Billing::Invoice");
}

#[test]
fn a_detached_comment_is_not_a_doc() {
    let got = chunks("# frozen_string_literal: true\n\nclass Invoice\n  def total\n  end\nend\n");
    assert_eq!(
        got,
        [
            ("".into(), "# frozen_string_literal: true".into()),
            (
                "class Invoice".into(),
                "class Invoice\n  def total\n  end\nend".into()
            ),
        ]
    );
}

#[test]
fn a_script_without_definitions_is_chunked_like_text() {
    let got = chunks("puts \"hello\"\nputs \"world\"\n");
    assert_eq!(got, [("".into(), "puts \"hello\"\nputs \"world\"".into())]);
}

/// Lines of filler routes, enough to push a block over the chunk size.
fn routes(prefix: &str, n: usize, indent: &str) -> String {
    (0..n)
        .map(|i| format!("{indent}get :{prefix}_{i}, to: '{prefix}#page_{i}'\n"))
        .collect()
}

#[test]
fn routes_get_their_namespace_trail() {
    let source = format!(
        "Rails.application.routes.draw do\n{}  namespace :storefront do\n{}    namespace :account do\n      \
         root to: 'profiles#show'\n      post :cancel_membership, to: 'payment_methods#cancel_membership'\n    end\n  end\nend\n",
        routes("admin", 40, "  "),
        routes("page", 40, "    "),
    );

    let got = chunks(&source);

    let (section, text) = got
        .iter()
        .find(|(_, text)| text.contains("cancel_membership"))
        .expect("a chunk with the route");
    assert_eq!(
        section,
        "Rails.application.routes.draw > namespace :storefront"
    );
    assert!(text.contains("namespace :account do"), "{text}");
    assert!(
        got.iter()
            .all(|(s, _)| s.starts_with("Rails.application.routes.draw")),
        "{got:#?}"
    );
}

#[test]
fn specs_get_their_describe_trail() {
    let examples: String = (0..25)
        .map(|i| format!("    it \"handles case {i}\" do\n      expect(invoice.total).to eq({i})\n    end\n\n"))
        .collect();
    let source =
        format!("RSpec.describe Invoice do\n  context \"when paid\" do\n{examples}  end\nend\n");

    let got = chunks(&source);

    assert!(got.len() > 1, "{got:#?}");
    assert!(
        got.iter()
            .all(|(s, _)| s == "RSpec.describe Invoice > context \"when paid\""),
        "{got:#?}"
    );
}

#[test]
fn a_header_is_a_block_opener_only() {
    let lang = ruby_for_tests();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_wasm_store(WasmStore::new(&ENGINE).unwrap())
        .unwrap();
    parser.set_language(&lang.grammar).unwrap();
    for (source, want) in [
        (
            "namespace :account do\n  root to: 'a#b'\nend\n",
            Some("namespace :account"),
        ),
        (
            "items.each do |item|\n  item.save\nend\n",
            Some("items.each"),
        ),
        ("if paid?\n  ship\nend\n", None),
        ("get :x, to: 'a#b'\n", None),
    ] {
        let tree = parser.parse(source, None).unwrap();
        let node = tree.root_node().named_child(0).unwrap();
        assert_eq!(header(source, node).as_deref(), want, "{source:?}");
    }
}
