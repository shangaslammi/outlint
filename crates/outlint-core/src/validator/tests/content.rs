use crate::validator::content::{
    content_oracle_cell, prepare_content_edges, prepare_heading_edges, prepare_item_edges,
    PreparedContentScope, PreparedItemScope,
};
use crate::validator::prepare::ValidationPlan;
use crate::{load_schema, parse_markdown, Block, MarkdownOptions};

fn plan_and_document(schema: &str, markdown: &str) -> (crate::Schema, crate::Document) {
    let schema = load_schema(schema).expect("test schema is valid").schema;
    let document = parse_markdown(markdown, MarkdownOptions::default());
    (schema, document)
}

#[test]
fn one_of_reduces_to_match_and_minimum_cost_only() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - one_of:\n      - block: any\n      - block: list\n        list_kind: bullet\noutline: []\n",
        "- item\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let PreparedContentScope::Declared(rules) = &plan.content else {
        panic!("content is declared")
    };
    let (edges, work) =
        prepare_content_edges(document.preamble.as_slice(), rules).expect("content edges prepare");

    assert_eq!(edges.matches.matches(0, 0), Some(true));
    assert_eq!(edges.costs.cost(&edges.matches, 0, 0), Some(0));
    assert_eq!(work.content_predicates, 2);
    assert_eq!(work.choice_reductions, 1);
    let oracle = content_oracle_cell(&document.preamble.as_slice()[0], &rules[0]);
    assert_eq!(oracle.minimum_cost, Some(0));
    assert_eq!(oracle.first_minimum, Some(1));
    assert!(oracle.matched);
}

#[test]
fn no_text_matches_only_item_wildcard() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\n    items:\n      - match: '*'\n        repeat: 0..n\n      - match: ''\n        required: false\noutline: []\n",
        "-\n- []()\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let PreparedContentScope::Declared(content) = &plan.content else {
        panic!("content is declared")
    };
    let PreparedItemScope::Declared(items) = &content[0].items else {
        panic!("items are declared")
    };
    let Block::List(list) = &document.preamble.as_slice()[0] else {
        panic!("document contains a list")
    };
    let direct_items: Vec<_> = list.items.iter().cloned().collect();
    assert!(direct_items[0].text.is_none());
    let (edges, work) = prepare_item_edges(&direct_items, items).expect("item edges prepare");
    assert_eq!(edges.matches.matches(0, 0), Some(true));
    assert_eq!(edges.costs.cost(&edges.matches, 0, 0), Some(1));
    assert_eq!(edges.matches.matches(0, 1), Some(false));
    assert_eq!(edges.costs.cost(&edges.matches, 0, 1), None);
    assert!(direct_items
        .get(1)
        .and_then(|item| item.text.as_ref())
        .is_some_and(|text| text.text.is_empty()));
    assert_eq!(edges.matches.matches(1, 0), Some(true));
    assert_eq!(edges.matches.matches(1, 1), Some(true));
    assert_eq!(edges.costs.cost(&edges.matches, 1, 1), Some(0));
    assert_eq!(work.matcher_bytes, 0);
}

#[test]
fn all_three_domains_build_dimension_paired_edges() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\n    items:\n      - match: Item\n  - block: p\n    required: false\noutline:\n  - match: Heading\n",
        "- Item\n# Heading\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let heading =
        prepare_heading_edges(schema.outline(), 1, &[true]).expect("heading edges prepare");
    assert_eq!(heading.matches.matches(0, 0), Some(true));
    assert_eq!(heading.matches.matches(1, 0), None);
    assert_eq!(heading.costs.cost(&heading.matches, 0, 0), Some(0));

    let PreparedContentScope::Declared(content) = &plan.content else {
        panic!("content is declared")
    };
    let (blocks, _) = prepare_content_edges(document.preamble.as_slice(), content)
        .expect("content edges prepare");
    assert_eq!(blocks.matches.matches(0, 0), Some(true));
    assert_eq!(blocks.matches.matches(0, 1), Some(false));
    assert_eq!(blocks.matches.matches(0, 2), None);

    let PreparedItemScope::Declared(items) = &content[0].items else {
        panic!("items are declared")
    };
    let Block::List(list) = &document.preamble.as_slice()[0] else {
        panic!("document contains a list")
    };
    let direct_items: Vec<_> = list.items.iter().cloned().collect();
    let (item_edges, _) = prepare_item_edges(&direct_items, items).expect("item edges prepare");
    assert_eq!(item_edges.matches.matches(0, 0), Some(true));
    assert_eq!(item_edges.costs.cost(&item_edges.matches, 0, 0), Some(0));
    assert_eq!(item_edges.costs.cost(&blocks.matches, 0, 0), None);
}
