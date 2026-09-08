use crate::validator::content::{
    content_oracle_cell, prepare_content_edges, prepare_heading_edges, prepare_item_edges,
    PreparedContentScope, PreparedItemScope,
};
use crate::validator::engine::{validation_scope_state, ScopeCounts};
use crate::validator::prepare::ValidationPlan;
use crate::validator::sequence::assign;
use crate::validator::{DiagnosticId, DiagnosticTarget, HeaderPath, ListAddress};
use crate::{
    load_schema, parse_markdown, Block, BlockKind, ContentOwner, ContentRuleIndex, ContentRulePath,
    ItemRuleIndex, ItemRulePath, MarkdownOptions, RuleIndex, RulePath, SchemaNode, ScopePath,
};

use super::diagnostics;

fn plan_and_document(schema: &str, markdown: &str) -> (crate::Schema, crate::Document) {
    let schema = load_schema(schema).expect("test schema is valid").schema;
    let document =
        parse_markdown(markdown, MarkdownOptions::default()).expect("Markdown parsing succeeds");
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
fn outer_one_of_is_greedy_in_canonical_assignment() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - one_of: [{block: p}, {block: list}]\n    repeat: 0..n\n  - block: p\n    repeat: 0..n\noutline: []\n",
        "first\n\nsecond\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let PreparedContentScope::Declared(rules) = &plan.content else {
        panic!("content is declared")
    };
    let (edges, _) =
        prepare_content_edges(document.preamble.as_slice(), rules).expect("content edges prepare");
    let assignment =
        assign(&edges.rules, &edges.matches, &edges.costs).expect("canonical assignment succeeds");

    assert!(assignment.accepted);
    assert_eq!(assignment.counts, [2, 0]);
    assert_eq!(assignment.rules, [Some(0), Some(0)]);
}

#[test]
fn overlapping_choices_and_adjacent_variable_repeats_reduce_once() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - one_of: [{block: any}, {block: p}]\n    repeat: 0..n\n  - one_of: [{block: any}, {block: list, list_kind: bullet}]\n    repeat: 0..4294967295\n  - block: any\n    repeat: 0..n\noutline: []\n",
        "paragraph\n\n- bullet\n\nsecond paragraph\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let PreparedContentScope::Declared(rules) = &plan.content else {
        panic!("content is declared")
    };
    let (edges, work) =
        prepare_content_edges(document.preamble.as_slice(), rules).expect("edges prepare");
    let assignment =
        assign(&edges.rules, &edges.matches, &edges.costs).expect("assignment completes");

    // The first two cells on each applicable choice use their specific arm's
    // cost, while the outer choices remain greedy phases. The huge maximum is
    // clamped to B and never creates occurrence states.
    assert_eq!(work.content_predicates, 3 * 5);
    assert_eq!(work.choice_reductions, 3 * 3);
    assert_eq!(edges.costs.cost(&edges.matches, 0, 0), Some(0));
    assert_eq!(edges.costs.cost(&edges.matches, 1, 1), Some(0));
    assert_eq!(edges.costs.cost(&edges.matches, 2, 2), Some(1));
    assert!(assignment.accepted);
    // Both [3,0,0] and [1,1,1] cost one; the first outer choice is specific
    // (and therefore greedy), even where its winning arm is `any`.
    assert_eq!(assignment.counts, [3, 0, 0]);
}

#[test]
fn preparation_counters_vary_b_c_a_i_j_independently() {
    fn content_work(blocks: usize, rules: usize, alternatives: usize) -> (u64, u64) {
        let arms = [
            "{block: p}",
            "{block: any}",
            "{block: list, list_kind: bullet}",
            "{block: list, list_kind: ordered}",
        ];
        let mut schema = "version: 1\ncontent:\n".to_owned();
        for rule_index in 0..rules {
            if rule_index == 0 && alternatives > 1 {
                schema.push_str("  - one_of: [");
                schema.push_str(&arms[..alternatives].join(", "));
                schema.push_str("]\n    repeat: 0..n\n");
            } else {
                schema.push_str("  - block: p\n    repeat: 0..n\n");
            }
        }
        schema.push_str("outline: []\n");
        let markdown = (0..blocks)
            .map(|index| format!("block {index}\n"))
            .collect::<Vec<_>>()
            .join("\n");
        let (schema, document) = plan_and_document(&schema, &markdown);
        let plan = ValidationPlan::new(&schema).expect("schema prepares");
        let PreparedContentScope::Declared(prepared) = &plan.content else {
            panic!("content is declared")
        };
        let (_, work) = prepare_content_edges(document.preamble.as_slice(), prepared)
            .expect("content edges prepare once");
        (work.content_predicates, work.choice_reductions)
    }

    for (blocks, rules, alternatives) in [
        (0usize, 2usize, 2usize),
        (1, 2, 2),
        (4, 2, 2),
        (4, 1, 1),
        (4, 3, 1),
        (4, 3, 2),
        (4, 1, 4),
    ] {
        let total_alternatives = alternatives + rules.saturating_sub(1);
        assert_eq!(
            content_work(blocks, rules, alternatives),
            (
                (blocks * total_alternatives) as u64,
                (blocks * rules) as u64,
            )
        );
    }

    for (items, rules) in [(0, 2), (1, 2), (5, 2), (5, 0), (5, 1), (5, 4)] {
        let mut schema = "version: 1\ncontent:\n  - block: list\n".to_owned();
        if rules == 0 {
            schema.push_str("    items: []\n");
        } else {
            schema.push_str("    items:\n");
            for _ in 0..rules {
                schema.push_str("      - match: X\n        repeat: 0..n\n");
            }
        }
        schema.push_str("outline: []\n");
        let markdown = (0..items.max(1)).map(|_| "- X\n").collect::<String>();
        let (schema, document) = plan_and_document(&schema, &markdown);
        let plan = ValidationPlan::new(&schema).expect("schema prepares");
        let PreparedContentScope::Declared(content) = &plan.content else {
            panic!("content is declared")
        };
        let PreparedItemScope::Declared(item_rules) = &content[0].items else {
            panic!("items are declared")
        };
        let Block::List(list) = &document.preamble.as_slice()[0] else {
            panic!("document contains one list")
        };
        let direct_items = list.items.iter().take(items).collect::<Vec<_>>();
        let (_, work) =
            prepare_item_edges(&direct_items, item_rules).expect("item edges prepare once");
        assert_eq!(work.matcher_bytes, (items * rules) as u64);
    }
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
    let direct_items: Vec<_> = list.items.iter().collect();
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
        prepare_heading_edges(schema.outline(), 1, vec![true]).expect("heading edges prepare");
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
    let direct_items: Vec<_> = list.items.iter().collect();
    let (item_edges, _) = prepare_item_edges(&direct_items, items).expect("item edges prepare");
    assert_eq!(item_edges.matches.matches(0, 0), Some(true));
    assert_eq!(item_edges.costs.cost(&item_edges.matches, 0, 0), Some(0));
    assert_eq!(item_edges.costs.cost(&blocks.matches, 0, 0), None);
}

#[test]
fn omitted_and_declared_empty_scopes_take_distinct_paths() {
    let omitted = load_schema("version: 1\noutline: []\n").expect("schema is valid");
    let declared = load_schema("version: 1\ncontent: []\noutline: []\n").expect("schema is valid");
    let document = parse_markdown("Visible paragraph\n", MarkdownOptions::default())
        .expect("Markdown parsing succeeds");
    let omitted_plan = ValidationPlan::new(&omitted.schema).expect("schema prepares");
    let declared_plan = ValidationPlan::new(&declared.schema).expect("schema prepares");

    let (omitted_work, omitted_counts, _) =
        validation_scope_state(&omitted.schema, &document, &omitted_plan)
            .expect("validation completes");
    let (declared_work, declared_counts, _) =
        validation_scope_state(&declared.schema, &document, &declared_plan)
            .expect("validation completes");

    assert_eq!(omitted_counts.content, 0);
    assert_eq!(declared_counts.content, 1);
    assert_eq!(omitted_work.content_predicates, 0);
    assert_eq!(omitted_work.choice_reductions, 0);
    assert_eq!(omitted_work.dp_cells, 1); // the declared empty outline only
    assert_eq!(declared_work.content_predicates, 0);
    assert_eq!(declared_work.choice_reductions, 0);
    assert_eq!(declared_work.dp_cells, 5); // outline 1 plus failed content 2*1*2
    assert_eq!(omitted_counts.headings, 1);
    assert_eq!(declared_counts.headings, 1);

    let (items_omitted, list_document) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\noutline: []\n",
        "- item\n",
    );
    let (items_empty, _) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\n    items: []\noutline: []\n",
        "- item\n",
    );
    let omitted_plan = ValidationPlan::new(&items_omitted).expect("schema prepares");
    let empty_plan = ValidationPlan::new(&items_empty).expect("schema prepares");
    let (omitted_work, omitted_counts, _) =
        validation_scope_state(&items_omitted, &list_document, &omitted_plan)
            .expect("validation completes");
    let (empty_work, empty_counts, _) =
        validation_scope_state(&items_empty, &list_document, &empty_plan)
            .expect("validation completes");

    assert_eq!(omitted_counts.items, 0);
    assert_eq!(empty_counts.items, 1);
    assert_eq!(omitted_work.dp_cells, 5); // content 2*2 plus outline 1
    assert_eq!(empty_work.dp_cells, 9); // plus failed empty items 2*1*2
}

#[test]
fn recovery_excess_lists_open_items() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\n    items: []\noutline: []\n",
        "- bullet\n\n1. ordered\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let (_, counts, _) =
        validation_scope_state(&schema, &document, &plan).expect("validation completes");

    assert_eq!(document.preamble.len(), 2);
    assert_eq!(counts.content, 1);
    assert_eq!(counts.items, 2);
}

#[test]
fn unassigned_lists_do_not_open_items() {
    let (schema, document) = plan_and_document(
        "version: 1\ncontent:\n  - block: list\n    list_kind: ordered\n    items: []\noutline: []\n",
        "- bullet\n",
    );
    let plan = ValidationPlan::new(&schema).expect("schema prepares");
    let (_, counts, _) =
        validation_scope_state(&schema, &document, &plan).expect("validation completes");

    assert_eq!(
        counts,
        ScopeCounts {
            headings: 1,
            content: 1,
            items: 0,
        }
    );
}

#[test]
fn sequence_cardinality_emits_one_diagnostic_per_rule() {
    let reported = diagnostics(
        "version: 1\ncontent:\n  - block: p\n    repeat: 2..3\n  - block: list\n    repeat: 2..3\n    items:\n      - match: A\n        repeat: 2..3\noutline:\n  - match: Heading\n    repeat: 2..3\n",
        "paragraph\n\n- A\n# Heading\n",
    );
    let ids = reported
        .iter()
        .map(|diagnostic| diagnostic.id)
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        [
            DiagnosticId::TooFewBlocks,
            DiagnosticId::TooFewBlocks,
            DiagnosticId::TooFewItems,
            DiagnosticId::TooFewSections,
        ]
    );
    assert_eq!(
        reported
            .iter()
            .filter(|diagnostic| diagnostic.id == DiagnosticId::TooFewBlocks)
            .count(),
        2
    );
}

#[test]
fn sequence_cardinality_anchors_first_excess_occurrence() {
    let blocks = diagnostics(
        "version: 1\ncontent:\n  - block: p\n    repeat: 0..1\noutline: []\n",
        "first\n\nsecond\n\nthird\n",
    );
    let block = blocks
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::TooManyBlocks)
        .expect("an excess block is reported");
    assert_eq!(block.location.line, 3);
    assert_eq!(
        block.target,
        DiagnosticTarget::Block {
            parent: HeaderPath::default(),
            block: BlockKind::Paragraph,
            index: 1,
        }
    );

    let items = diagnostics(
        "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n        repeat: 0..1\noutline: []\n",
        "- A\n- A\n- A\n",
    );
    let item = items
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::TooManyItems)
        .expect("an excess item is reported");
    assert_eq!(item.location.line, 2);
    assert_eq!(
        item.target,
        DiagnosticTarget::Item {
            list: ListAddress {
                parent: HeaderPath::default(),
                index: 0,
            },
            index: 1,
        }
    );
}

#[test]
fn recovery_attribution_uses_owner_and_rule_nodes() {
    let root = diagnostics("version: 1\ncontent: []\noutline: []\n", "root\n");
    assert_eq!(root[0].id, DiagnosticId::UnexpectedBlock);
    assert_eq!(root[0].schema_node, None);

    let section = diagnostics(
        "version: 1\noutline:\n  - match: Parent\n    content: []\n",
        "# Parent\n\nchild block\n",
    );
    let block = section
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::UnexpectedBlock)
        .expect("the section block is unassigned");
    assert_eq!(
        block.schema_node,
        Some(SchemaNode::Rule(RulePath {
            scope: ScopePath(Vec::new()),
            index: RuleIndex(0),
        }))
    );

    let item = diagnostics(
        "version: 1\ncontent:\n  - block: list\n    items: []\noutline: []\n",
        "- item\n",
    );
    assert_eq!(item[0].id, DiagnosticId::UnexpectedItem);
    assert_eq!(
        item[0].schema_node,
        Some(SchemaNode::ContentRule(ContentRulePath {
            owner: ContentOwner::Document,
            index: ContentRuleIndex(0),
        }))
    );

    let cardinality = diagnostics(
        "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n        repeat: 2..2\noutline: []\n",
        "- A\n",
    );
    assert_eq!(
        cardinality[0].schema_node,
        Some(SchemaNode::ItemRule(ItemRulePath {
            content: ContentRulePath {
                owner: ContentOwner::Document,
                index: ContentRuleIndex(0),
            },
            index: ItemRuleIndex(0),
        }))
    );

    let misplaced_block = diagnostics(
        "version: 1\ncontent:\n  - block: p\n  - block: list\noutline: []\n",
        "- first\n\nparagraph\n",
    );
    let misplaced = misplaced_block
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::MisplacedBlock)
        .expect("a matching out-of-phase block is misplaced");
    assert_eq!(misplaced.schema_node, None);
    let missing = misplaced_block
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::MissingBlock)
        .expect("the empty content phase is missing");
    assert_eq!(
        missing.schema_node,
        Some(SchemaNode::ContentRule(ContentRulePath {
            owner: ContentOwner::Document,
            index: ContentRuleIndex(1),
        }))
    );

    let misplaced_item = diagnostics(
        "version: 1\ncontent:\n  - block: list\n    items:\n      - match: A\n      - match: B\noutline: []\n",
        "- B\n- A\n",
    );
    let misplaced = misplaced_item
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::MisplacedItem)
        .expect("a matching out-of-phase item is misplaced");
    assert_eq!(
        misplaced.schema_node,
        Some(SchemaNode::ContentRule(ContentRulePath {
            owner: ContentOwner::Document,
            index: ContentRuleIndex(0),
        }))
    );
    let missing = misplaced_item
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::MissingItem)
        .expect("the empty item phase is missing");
    assert_eq!(
        missing.schema_node,
        Some(SchemaNode::ItemRule(ItemRulePath {
            content: ContentRulePath {
                owner: ContentOwner::Document,
                index: ContentRuleIndex(0),
            },
            index: ItemRuleIndex(1),
        }))
    );

    let title = diagnostics(
        "version: 1\ntitle: Doc\ncontent: []\nsections: []\n",
        "# Doc\n\nvisible\n",
    );
    let title_block = title
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::UnexpectedBlock)
        .expect("title recovery is reported");
    assert_eq!(title_block.schema_node, Some(SchemaNode::Title));
    assert!(matches!(
        &title_block.target,
        DiagnosticTarget::Block { parent, .. } if parent == &HeaderPath(vec!["Doc".into()])
    ));

    let single_title_missing = diagnostics(
        "version: 1\ntitle: Doc\ncontent:\n  - block: p\nsections: []\n",
        "\n\n# Doc\n",
    );
    let missing = single_title_missing
        .iter()
        .find(|diagnostic| diagnostic.id == DiagnosticId::MissingBlock)
        .expect("the title preamble is missing a paragraph");
    assert_eq!(missing.location.line, 1);
    assert!(matches!(
        &missing.target,
        DiagnosticTarget::MissingBlock { parent, .. } if parent == &HeaderPath::default()
    ));
    assert_eq!(
        missing.schema_node,
        Some(SchemaNode::ContentRule(ContentRulePath {
            owner: ContentOwner::Title,
            index: ContentRuleIndex(0),
        }))
    );
}

#[test]
fn targets_use_precomputed_concrete_ordinals() {
    let blocks = diagnostics(
        "version: 1\ncontent: []\noutline: []\n",
        "first\n\n- a\n\nsecond\n\n1. b\n",
    );
    let targets = blocks
        .iter()
        .map(|diagnostic| diagnostic.target.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            DiagnosticTarget::Block {
                parent: HeaderPath::default(),
                block: BlockKind::Paragraph,
                index: 0,
            },
            DiagnosticTarget::Block {
                parent: HeaderPath::default(),
                block: BlockKind::List,
                index: 0,
            },
            DiagnosticTarget::Block {
                parent: HeaderPath::default(),
                block: BlockKind::Paragraph,
                index: 1,
            },
            DiagnosticTarget::Block {
                parent: HeaderPath::default(),
                block: BlockKind::List,
                index: 1,
            },
        ]
    );

    let items = diagnostics(
        "version: 1\ncontent:\n  - block: list\n    repeat: 0..n\n    items: []\noutline: []\n",
        "- first\n- second\n\n1. third\n",
    );
    let addresses = items
        .iter()
        .map(|diagnostic| diagnostic.target.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        addresses,
        [
            DiagnosticTarget::Item {
                list: ListAddress {
                    parent: HeaderPath::default(),
                    index: 0,
                },
                index: 0,
            },
            DiagnosticTarget::Item {
                list: ListAddress {
                    parent: HeaderPath::default(),
                    index: 0,
                },
                index: 1,
            },
            DiagnosticTarget::Item {
                list: ListAddress {
                    parent: HeaderPath::default(),
                    index: 1,
                },
                index: 0,
            },
        ]
    );
}
