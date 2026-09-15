//! Unit tests for document path parsing, rendering, resolution, and
//! enumeration.

use super::{
    document_paths, heading_slug, BlockPathKind, BlockStep, DocumentNode, DocumentPath,
    DocumentPathError, HeadingSlug, SectionStep,
};
use crate::{parse_markdown, Block, ByteOffset, Document, MarkdownOptions, TextRange};

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
        resolve(&document, "$.guide.faq.question"),
        Err(DocumentPathError::Ambiguous {
            resolved_steps: 2,
            candidates: 2
        })
    );
    let Ok(DocumentNode::Section(second)) = resolve(&document, "$.guide.faq.question[1]") else {
        panic!("`question[1]` selects the second duplicate")
    };
    assert_eq!(second.heading.text, "Question");
    assert!(second.preamble.len() == 1);
    assert_eq!(
        range(DocumentNode::Section(second)),
        range(resolve(&document, "$.guide.faq.[1]").expect("positional step resolves"))
    );
}

#[test]
fn slugless_headings_resolve_only_positionally() {
    let document = document();
    assert_eq!(heading_slug("🎉"), None);
    let Ok(DocumentNode::Section(party)) = resolve(&document, "$.guide.faq.[2]") else {
        panic!("the emoji heading is the third FAQ child")
    };
    assert_eq!(party.heading.text, "🎉");
    assert_eq!(
        resolve(&document, "$.guide.faq.[3]"),
        Err(DocumentPathError::Unresolved { resolved_steps: 2 })
    );
}

#[test]
fn block_ordinals_count_per_kind_and_items_follow_lists() {
    let document = document();
    let setup = match resolve(&document, "$.guide.setup") {
        Ok(DocumentNode::Section(section)) => section,
        other => panic!("setup section: {other:?}"),
    };
    let blocks = setup.preamble.as_slice();
    assert_eq!(blocks.len(), 3);

    let Ok(DocumentNode::Block(second_paragraph)) = resolve(&document, "$.guide.setup/p[1]") else {
        panic!("`p[1]` skips the interleaved list")
    };
    assert_eq!(
        range(DocumentNode::Block(second_paragraph)),
        blocks
            .get(2)
            .map(|block| range(DocumentNode::Block(block)).expect("block range"))
    );

    let Ok(DocumentNode::Item(third)) = resolve(&document, "$.guide.setup/list[0]/item[2]") else {
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
        ("$.guide.setup/table[0]", 2),
        ("$.guide.setup/p[2]", 2),
        ("$.guide.setup/list[0]/item[3]", 3),
        ("$.guide.setup/list[0]/item[0]/p[0]", 4),
        ("$.guide.missing", 1),
        ("$.[1]", 0),
    ] {
        assert_eq!(
            resolve(&document, spelling),
            Err(DocumentPathError::Unresolved { resolved_steps }),
            "{spelling}"
        );
    }
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
            "$.guide",
            "$.guide.faq",
            "$.guide.faq.question[0]",
            "$.guide.faq.question[0]/p[0]",
            "$.guide.faq.question[1]",
            "$.guide.faq.question[1]/p[0]",
            "$.guide.faq.[2]",
            "$.guide.faq.[2]/p[0]",
            "$.guide.setup",
            "$.guide.setup/p[0]",
            "$.guide.setup/list[0]",
            "$.guide.setup/list[0]/item[0]",
            "$.guide.setup/list[0]/item[1]",
            "$.guide.setup/list[0]/item[2]",
            "$.guide.setup/p[1]",
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
