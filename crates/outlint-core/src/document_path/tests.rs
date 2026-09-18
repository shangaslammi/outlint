//! Unit tests for document path parsing, rendering, resolution, and
//! enumeration.

use super::{
    document_paths, heading_slug, merged_title, BlockPathKind, BlockStep, DocumentNode,
    DocumentPath, DocumentPathError, HeadingSlug, SectionStep,
};
use crate::{parse_markdown, Block, ByteOffset, Document, HeaderLevel, MarkdownOptions, TextRange};

const SOURCE: &str = "\
Intro paragraph.

- root item a
- root item b

# Guide

## FAQ

### Question

first answer

### Question

second answer

### 🎉

party

## Setup

Para zero.

- one
- two
- three

Para one.
";

fn document() -> Document {
    parse_markdown(SOURCE, MarkdownOptions::default()).expect("the fixture parses")
}

fn parse(spelling: &str) -> DocumentPath {
    DocumentPath::parse(spelling).unwrap_or_else(|error| panic!("{spelling}: {error}"))
}

fn resolve<'d>(
    document: &'d Document,
    spelling: &str,
) -> Result<DocumentNode<'d>, DocumentPathError> {
    parse(spelling).resolve(document)
}

/// The source range that identifies a node in comparisons; `None` for the root.
fn range(node: DocumentNode<'_>) -> Option<TextRange> {
    match node {
        DocumentNode::Root(_) => None,
        DocumentNode::Section(section) => Some(section.heading.location.range),
        DocumentNode::Block(Block::List(list)) => Some(list.location.range),
        DocumentNode::Block(
            Block::Paragraph(leaf)
            | Block::Quote(leaf)
            | Block::Code(leaf)
            | Block::Html(leaf)
            | Block::Break(leaf),
        ) => Some(leaf.location.range),
        DocumentNode::Item(item) => Some(item.location.range),
    }
}

fn slug(text: &str) -> HeadingSlug {
    HeadingSlug::parse(text).expect("literal slug is well-formed")
}

#[test]
fn every_segment_form_round_trips_through_its_canonical_spelling() {
    for spelling in [
        "$",
        "$.deployment.rollback-plan",
        "$.faq.question[2]",
        "$.[2]",
        "$.setup/p[0]",
        "$.setup/list[0]/item[3]",
        "$/p[0]",
        "$.api/table[0]/row[2]/cell[1]",
        "$..a",
        "$.a..b[1]/p[0]",
    ] {
        let path = parse(spelling);
        assert_eq!(path.to_string(), spelling);
        assert_eq!(spelling.parse::<DocumentPath>().as_ref(), Ok(&path));
    }
}

#[test]
fn an_omitted_block_index_defaults_to_zero() {
    let path = parse("$.setup/list/item");
    assert_eq!(path.to_string(), "$.setup/list[0]/item[0]");
    assert_eq!(
        path.sections(),
        [SectionStep::Named {
            slug: slug("setup"),
            index: None
        }]
    );
    assert_eq!(
        path.blocks(),
        [
            BlockStep {
                kind: BlockPathKind::List,
                index: 0
            },
            BlockStep {
                kind: BlockPathKind::Item,
                index: 0
            },
        ]
    );
}

#[test]
fn rejects_malformed_spellings_at_the_offending_byte() {
    for (spelling, offset) in [
        (".setup", 0),
        ("$.setup[]", 8),
        ("$.setup[01]", 8),
        ("$/item[0]", 2),
        ("$/frob", 2),
        ("$.setup.", 8),
        ("$.setup/list[0]/item[0]/item[0]", 24),
        ("$.-setup", 2),
        ("$.a--b", 4),
        ("$.a-", 4),
        ("$..[0]", 3),
        ("$..", 3),
        ("$...a", 3),
    ] {
        let error = DocumentPath::parse(spelling).expect_err(spelling);
        assert_eq!(error.offset, ByteOffset(offset), "{spelling}: {error}");
    }
}

#[test]
fn builders_keep_item_steps_behind_list_steps() {
    let item = BlockStep {
        kind: BlockPathKind::Item,
        index: 0,
    };
    let list = BlockStep {
        kind: BlockPathKind::List,
        index: 0,
    };
    assert_eq!(DocumentPath::root().with_block(item), None);
    let path = DocumentPath::root()
        .with_section(SectionStep::Position(0))
        .and_then(|path| path.with_block(list))
        .and_then(|path| path.with_block(item))
        .expect("list then item is well-formed");
    assert_eq!(path.to_string(), "$.[0]/list[0]/item[0]");
    assert_eq!(path.with_section(SectionStep::Position(1)), None);
}

#[test]
fn duplicate_sibling_slugs_need_an_index() {
    let document = document();
    assert_eq!(
        resolve(&document, "$.faq.question"),
        Err(DocumentPathError::Ambiguous {
            resolved_steps: 1,
            candidates: 2
        })
    );
    let Ok(DocumentNode::Section(second)) = resolve(&document, "$.faq.question[1]") else {
        panic!("`question[1]` selects the second duplicate")
    };
    assert_eq!(second.heading.text, "Question");
    assert!(second.preamble.len() == 1);
    assert_eq!(
        range(DocumentNode::Section(second)),
        range(resolve(&document, "$.faq.[1]").expect("positional step resolves"))
    );
}

#[test]
fn slugless_headings_resolve_only_positionally() {
    let document = document();
    assert_eq!(heading_slug("🎉"), None);
    let Ok(DocumentNode::Section(party)) = resolve(&document, "$.faq.[2]") else {
        panic!("the emoji heading is the third FAQ child")
    };
    assert_eq!(party.heading.text, "🎉");
    assert_eq!(
        resolve(&document, "$.faq.[3]"),
        Err(DocumentPathError::Unresolved { resolved_steps: 1 })
    );
}

#[test]
fn block_ordinals_count_per_kind_and_items_follow_lists() {
    let document = document();
    let setup = match resolve(&document, "$.setup") {
        Ok(DocumentNode::Section(section)) => section,
        other => panic!("setup section: {other:?}"),
    };
    let blocks = setup.preamble.as_slice();
    assert_eq!(blocks.len(), 3);

    let Ok(DocumentNode::Block(second_paragraph)) = resolve(&document, "$.setup/p[1]") else {
        panic!("`p[1]` skips the interleaved list")
    };
    assert_eq!(
        range(DocumentNode::Block(second_paragraph)),
        blocks
            .get(2)
            .map(|block| range(DocumentNode::Block(block)).expect("block range"))
    );

    let Ok(DocumentNode::Item(third)) = resolve(&document, "$.setup/list[0]/item[2]") else {
        panic!("`item[2]` is the third item")
    };
    assert_eq!(
        third.text.as_ref().map(|text| text.text.as_str()),
        Some("three")
    );

    let Ok(DocumentNode::Item(root_item)) = resolve(&document, "$/list[0]/item[1]") else {
        panic!("the root preamble owns the first list")
    };
    assert_eq!(
        root_item.text.as_ref().map(|text| text.text.as_str()),
        Some("root item b")
    );
    assert!(matches!(
        resolve(&document, "$/p[0]"),
        Ok(DocumentNode::Block(Block::Paragraph(_)))
    ));
    assert!(matches!(resolve(&document, "$"), Ok(DocumentNode::Root(_))));
}

#[test]
fn unanswerable_steps_are_unresolved_after_the_deepest_reached_node() {
    let document = document();
    for (spelling, resolved_steps) in [
        ("$.setup/table[0]", 1),
        ("$.setup/p[2]", 1),
        ("$.setup/list[0]/item[3]", 2),
        ("$.setup/list[0]/item[0]/p[0]", 3),
        ("$.faq.missing", 1),
        ("$.[2]", 0),
        // The merged H1 has no address of its own.
        ("$.guide.setup", 0),
    ] {
        assert_eq!(
            resolve(&document, spelling),
            Err(DocumentPathError::Unresolved { resolved_steps }),
            "{spelling}"
        );
    }
}

#[test]
fn section_extents_span_their_subtrees_and_blocks_their_ranges() {
    let document = document();
    let extent = |spelling: &str| {
        resolve(&document, spelling)
            .unwrap_or_else(|error| panic!("{spelling}: {error}"))
            .extent()
    };
    let range_of = |spelling: &str| range(resolve(&document, spelling).expect(spelling));

    assert_eq!(extent("$"), None);
    // A parent section reaches past its last descendant's heading to that
    // descendant's last block.
    let faq = extent("$.faq").expect("section extent");
    assert_eq!(Some(faq.start), range_of("$.faq").map(|range| range.start));
    assert!(range_of("$.faq.[2]").is_some_and(|heading| heading.end < faq.end));
    assert_eq!(
        Some(faq.end),
        range_of("$.faq.[2]/p[0]").map(|range| range.end)
    );
    // A leaf section ends at its last own block, not at the next heading.
    let question = extent("$.faq.question[0]").expect("section extent");
    assert_eq!(
        Some(question.end),
        range_of("$.faq.question[0]/p[0]").map(|range| range.end)
    );
    assert!(range_of("$.faq.question[1]").is_some_and(|next| question.end < next.start));
    assert_eq!(extent("$.setup/list[0]"), range_of("$.setup/list[0]"));
    assert_eq!(
        extent("$.setup/list[0]/item[2]"),
        range_of("$.setup/list[0]/item[2]")
    );
}

const TITLED: &str = "\
---
title: Titled
---

Root paragraph.

# Title

Title paragraph.

## A

## B
";

#[test]
fn a_sole_h1_is_merged_into_the_root() {
    let document = parse_markdown(TITLED, MarkdownOptions::default()).expect("the fixture parses");
    let spellings: Vec<String> = document_paths(&document)
        .iter()
        .map(|(path, _)| path.to_string())
        .collect();
    assert_eq!(spellings, ["$", "$/p[0]", "$/p[1]", "$.a", "$.b"]);
    assert_eq!(
        merged_title(&document).map(|title| title.heading.text.as_str()),
        Some("Title")
    );
    // The title's own blocks continue the root's per-kind ordinals.
    let Ok(DocumentNode::Block(Block::Paragraph(second))) = resolve(&document, "$/p[1]") else {
        panic!("`p[1]` is the paragraph after the title")
    };
    assert_eq!(
        TITLED.get(second.location.range.start.0..second.location.range.end.0),
        Some("Title paragraph.\n")
    );
    assert_eq!(
        resolve(&document, "$.title.a"),
        Err(DocumentPathError::Unresolved { resolved_steps: 0 })
    );
    assert!(matches!(resolve(&document, "$"), Ok(DocumentNode::Root(_))));

    // Several top-level sections, or a sole one below H1, are not merged.
    for (source, path) in [
        ("# First\n\n## A\n\n# Second\n", "$.first.a"),
        ("## Only\n\n### A\n", "$.only.a"),
    ] {
        let document = parse_markdown(source, MarkdownOptions::default()).expect("parses");
        assert_eq!(merged_title(&document), None, "{source:?}");
        assert!(
            matches!(resolve(&document, path), Ok(DocumentNode::Section(_))),
            "{source:?}: {path}"
        );
    }
}

#[test]
fn descendant_steps_search_the_current_subtree() {
    const NESTED: &str = "# Doc\n\n## Question\n\n### Question\n\n## B\n\n### Deep\n";
    let document = parse_markdown(NESTED, MarkdownOptions::default()).expect("parses");
    let heading = |spelling: &str| match resolve(&document, spelling) {
        Ok(DocumentNode::Section(section)) => Ok(section.heading.level),
        Ok(other) => panic!("{spelling}: {other:?}"),
        Err(error) => Err(error),
    };
    // Unique anywhere below the root.
    assert_eq!(heading("$..deep"), Ok(HeaderLevel::H3));
    // Two matches at different depths: ambiguous without an index, positional
    // in document order with one.
    assert_eq!(
        heading("$..question"),
        Err(DocumentPathError::Ambiguous {
            resolved_steps: 0,
            candidates: 2
        })
    );
    assert_eq!(heading("$..question[1]"), Ok(HeaderLevel::H3));
    assert_eq!(heading("$.question..question"), Ok(HeaderLevel::H3));
    // Scoped to the subtree of the node reached so far.
    assert_eq!(
        heading("$.b..question"),
        Err(DocumentPathError::Unresolved { resolved_steps: 1 })
    );
}

#[test]
fn shared_sibling_slugs_are_indexed_in_document_order() {
    let document = parse_markdown("# A\n# B\n# A\n# C\n# A\n", MarkdownOptions::default())
        .expect("the fixture parses");
    let spellings: Vec<String> = document_paths(&document)
        .iter()
        .map(|(path, _)| path.to_string())
        .collect();
    assert_eq!(spellings, ["$", "$.a[0]", "$.b", "$.a[1]", "$.c", "$.a[2]"]);
}

#[test]
fn enumerated_paths_render_reparse_and_resolve_to_their_nodes() {
    let document = document();
    let paths = document_paths(&document);
    let spellings: Vec<String> = paths.iter().map(|(path, _)| path.to_string()).collect();
    assert_eq!(
        spellings,
        [
            "$",
            "$/p[0]",
            "$/list[0]",
            "$/list[0]/item[0]",
            "$/list[0]/item[1]",
            "$.faq",
            "$.faq.question[0]",
            "$.faq.question[0]/p[0]",
            "$.faq.question[1]",
            "$.faq.question[1]/p[0]",
            "$.faq.[2]",
            "$.faq.[2]/p[0]",
            "$.setup",
            "$.setup/p[0]",
            "$.setup/list[0]",
            "$.setup/list[0]/item[0]",
            "$.setup/list[0]/item[1]",
            "$.setup/list[0]/item[2]",
            "$.setup/p[1]",
        ]
    );
    for ((path, node), spelling) in paths.iter().zip(&spellings) {
        let reparsed = parse(spelling);
        assert_eq!(&reparsed, path, "{spelling}");
        let resolved = reparsed
            .resolve(&document)
            .unwrap_or_else(|error| panic!("{spelling}: {error}"));
        assert_eq!(range(resolved), range(*node), "{spelling}");
        assert_eq!(
            std::mem::discriminant(&resolved),
            std::mem::discriminant(node),
            "{spelling}"
        );
    }
}
