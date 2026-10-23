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
