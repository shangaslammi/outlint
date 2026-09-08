use crate::markdown::lines::{text_range, LineIndex};
use crate::markdown::{
    parse_markdown, Block, BlockKind, BlockLocation, Document, DocumentFrontmatter, Heading,
    ItemText, ListBlock, ListKind, MarkdownOptions, Section,
};
use crate::HeaderLevel;
use pulldown_cmark::{Tag, TagEnd};

use super::super::body::{is_comment_only_html, scan_preambles, FrameStack};

fn block_kind(block: &Block) -> BlockKind {
    match block {
        Block::Paragraph(_) => BlockKind::Paragraph,
        Block::List(_) => BlockKind::List,
        Block::Quote(_) => BlockKind::Quote,
        Block::Code(_) => BlockKind::Code,
        Block::Html(_) => BlockKind::Html,
        Block::Break(_) => BlockKind::Break,
    }
}

fn block_location(block: &Block) -> &BlockLocation {
    match block {
        Block::Paragraph(block)
        | Block::Quote(block)
        | Block::Code(block)
        | Block::Html(block)
        | Block::Break(block) => &block.location,
        Block::List(block) => &block.location,
    }
}

fn lists(document: &Document) -> Vec<&ListBlock> {
    document
        .preamble
        .iter()
        .filter_map(|block| match block {
            Block::List(list) => Some(list),
            _ => None,
        })
        .collect()
}

fn item_text(item: &crate::markdown::ListItem) -> Option<&ItemText> {
    item.text.as_ref()
}

fn headings(document: &Document) -> Vec<&Heading> {
    fn visit<'a>(sections: &'a [Section], output: &mut Vec<&'a Heading>) {
        for section in sections {
            output.push(&section.heading);
            visit(&section.children, output);
        }
    }

    let mut output = Vec::new();
    visit(&document.sections, &mut output);
    output
}

#[test]
fn frame_stack_checks_nested_typed_closes_and_retains_ranges() {
    let mut frames = FrameStack::default();
    frames.push(Tag::BlockQuote(None), 2..40);
    frames.push(Tag::List(Some(1)), 5..35);
    frames.push(Tag::Item, 8..30);

    let item = frames
        .close(TagEnd::Item)
        .unwrap_or_else(|()| unreachable!());
    let list = frames
        .close(TagEnd::List(true))
        .unwrap_or_else(|()| unreachable!());
    let quote = frames
        .close(TagEnd::BlockQuote(None))
        .unwrap_or_else(|()| unreachable!());

    assert_eq!(item.expected_end, TagEnd::Item);
    assert_eq!(item.range, 8..30);
    assert_eq!(list.expected_end, TagEnd::List(true));
    assert_eq!(list.range, 5..35);
    assert_eq!(quote.expected_end, TagEnd::BlockQuote(None));
    assert_eq!(quote.range, 2..40);
    assert!(frames.is_top_level());
}

#[test]
fn frame_stack_mismatched_close_fails_closed_permanently() {
    let mut frames = FrameStack::default();
    frames.push(Tag::Paragraph, 4..12);

    assert!(frames.close(TagEnd::CodeBlock).is_err());
    assert!(!frames.is_top_level());
    assert!(frames.close(TagEnd::Paragraph).is_ok());
    assert!(!frames.is_top_level());
}

#[test]
fn frame_stack_checked_empty_close_fails_closed() {
    let mut frames = FrameStack::default();

    assert!(frames.is_top_level());
    assert!(frames.close(TagEnd::Paragraph).is_err());
    assert!(!frames.is_top_level());
}

#[test]
fn parses_atx_and_setext_headings_but_not_near_misses() {
    let source = concat!(
        "# one\n",
        "   ## two ##\n",
        "####### no\n\n",
        "    ### indented code\n\n",
        "no-space#\n\n",
        "setext one\n",
        "===\n",
        "setext two\n",
        "---\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| (heading.level, heading.text.as_str()))
        .collect();

    assert_eq!(
        actual,
        [
            (HeaderLevel::H1, "one"),
            (HeaderLevel::H2, "two"),
            (HeaderLevel::H1, "setext one"),
            (HeaderLevel::H2, "setext two"),
        ]
    );
}

#[test]
fn accepts_only_top_level_physical_heading_lines() {
    let source = concat!(
        "> # quoted atx\n\n",
        "- # listed atx\n\n",
        "> quoted setext\n> ===\n\n",
        "- listed setext\n  ---\n\n",
        "- containing item\n\n  ### continued-list atx\n\n",
        "#\ttab is not the required literal space\n\n",
        "   ## physical atx\n",
        "physical setext\n---\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| heading.text.as_str())
        .collect();

    assert_eq!(actual, ["physical atx", "physical setext"]);
}

#[test]
fn balanced_frames_preserve_heading_records() {
    let source = concat!(
        "<!-- outlint-disable skipped-level -->\n",
        "   # **Top &amp; one** #\r\n",
        "> quoted paragraph with *markup*\n>\n> ## nested\n\n",
        "Top `two`\n",
        "===\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let found = headings(&document);

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].level, HeaderLevel::H1);
    assert_eq!(found[0].text, "Top & one");
    assert_eq!(found[0].diagnostic_text, "Top & one");
    assert_eq!(found[0].source_text, "**Top &amp; one**");
    assert_eq!(found[0].location.line, 2);
    assert_eq!(found[0].location.column, 4);
    assert!(found[0].suppressions.contains("skipped-level"));
    assert_eq!(found[1].level, HeaderLevel::H1);
    assert_eq!(found[1].text, "Top two");
    assert_eq!(found[1].diagnostic_text, "Top two");
    assert_eq!(found[1].source_text, "Top `two`");
}

#[test]
fn nested_container_headings_remain_ineligible() {
    let source = concat!(
        "> # quote\n\n",
        "- ## list\n",
        "  - ### nested list\n\n",
        "> - #### list in quote\n\n",
        "<div>\n# html\n</div>\n\n",
        "```md\n# code\n```\n\n",
        "# top level\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| heading.text.as_str())
        .collect();

    assert_eq!(actual, ["top level"]);
}

#[test]
fn setext_and_atx_eligibility_is_unchanged() {
    let source = concat!(
        "# atx one\n",
        "   ## atx two ##\n",
        "setext one\n===\n",
        "   setext two\n   ---\n",
        "> nested setext\n> ===\n\n",
        "- nested atx\n\n  ### still nested\n\n",
        "    # indented code\n",
        "####### seven hashes\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| (heading.level, heading.text.as_str()))
        .collect();

    assert_eq!(
        actual,
        [
            (HeaderLevel::H1, "atx one"),
            (HeaderLevel::H2, "atx two"),
            (HeaderLevel::H1, "setext one"),
            (HeaderLevel::H2, "setext two"),
        ]
    );
}

#[test]
fn ignores_headings_in_commonmark_fences() {
    let source = concat!(
        "~~~ rust\n# hidden\n~~~\n",
        "   ```` language\n## also hidden\n``` not a close\n   ````\n",
        "### visible\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| heading.text.as_str())
        .collect();

    assert_eq!(actual, ["visible"]);
}

#[test]
fn applies_atx_closing_hash_rules() {
    let document = parse_markdown(
        "# text ###\n# text###\n# ###\n# text # tail\n",
        MarkdownOptions::default(),
    );
    let actual: Vec<_> = headings(&document)
        .into_iter()
        .map(|heading| (heading.text.as_str(), heading.source_text.as_str()))
        .collect();

    assert_eq!(
        actual,
        [
            ("text", "text"),
            ("text###", "text###"),
            ("", ""),
            ("text # tail", "text # tail"),
        ]
    );
}

#[test]
fn strips_inline_markup_and_decodes_commonmark_text() {
    let source = "## **A&amp;B** [link](target) ![alt](image) `code` <i>tag</i> \\*star\\*\n";
    let stripped = parse_markdown(source, MarkdownOptions::default());
    let preserved = parse_markdown(
        source,
        MarkdownOptions {
            strip_inline_markup: false,
        },
    );

    let stripped_heading = &stripped.sections[0].heading;
    assert_eq!(stripped_heading.text, "A&B link alt code tag *star*");
    assert_eq!(stripped_heading.diagnostic_text, stripped_heading.text);
    assert_eq!(
        stripped_heading.source_text,
        "**A&amp;B** [link](target) ![alt](image) `code` <i>tag</i> \\*star\\*"
    );
    assert_eq!(
        preserved.sections[0].heading.text,
        "**A&B** [link](target) ![alt](image) `code` <i>tag</i> *star*"
    );
}

#[test]
fn builds_tree_using_nearest_prior_lower_heading() {
    let document = parse_markdown(
        "# root\n### skipped\n#### child\n## sibling\n# next\n",
        MarkdownOptions::default(),
    );

    assert_eq!(document.sections.len(), 2);
    assert_eq!(document.sections[0].children.len(), 2);
    assert_eq!(document.sections[0].children[0].heading.text, "skipped");
    assert_eq!(document.sections[0].children[0].children.len(), 1);
    assert_eq!(document.sections[0].children[1].heading.text, "sibling");
}

#[test]
fn records_byte_line_column_and_setext_extent() {
    let source = "å\n\n   # atx\r\nsetext\n---\n";
    let document = parse_markdown(source, MarkdownOptions::default());
    let found = headings(&document);

    assert_eq!(found[0].location.line, 3);
    assert_eq!(found[0].location.column, 4);
    assert_eq!(found[0].location.line_range, text_range(4, 12));
    assert_eq!(found[1].location.line, 4);
    assert_eq!(
        source.get(found[1].location.range.start.0..found[1].location.range.end.0),
        Some("setext\n---\n")
    );
}

#[test]
fn captures_header_and_file_suppressions() {
    let source = concat!(
        "<!-- outlint-disable-file missing-section, requires -->\n",
        "<!-- outlint-disable skipped-level, not-allowed -->\n",
        "## suppressed\n",
        "<!-- outlint-disable unexpected-section -->\n",
        "\n",
        "## not suppressed\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let found = headings(&document);

    assert!(document.file_suppressions.contains("missing-section"));
    assert!(document.file_suppressions.contains("requires"));
    assert!(found[0].suppressions.contains("skipped-level"));
    assert!(found[0].suppressions.contains("not-allowed"));
    assert!(found[1].suppressions.0.is_empty());
}

#[test]
fn finds_file_suppressions_nested_in_raw_html() {
    let source = concat!(
        "<div>\n",
        "before\n",
        "<!-- outlint-disable-file missing-section -->\n",
        "<!-- outlint-disable-file requires, ordered -->\n",
        "after\n",
        "</div>\n\n",
        "# heading\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    assert!(document.file_suppressions.contains("missing-section"));
    assert!(document.file_suppressions.contains("requires"));
    assert!(document.file_suppressions.contains("ordered"));
}

#[test]
fn requires_header_suppression_to_occupy_its_whole_line() {
    let source = concat!(
        "prefix <!-- outlint-disable skipped-level -->\n",
        "# not suppressed\n",
        "<!-- outlint-disable skipped-level --> suffix\n",
        "# also not suppressed\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    assert!(headings(&document)
        .iter()
        .all(|heading| !heading.suppressions.contains("skipped-level")));
}

#[test]
fn bare_cr_delimits_locations_and_suppression_lines() {
    let source = concat!(
        "<!-- outlint-disable skipped-level -->\r",
        "   ## first\r",
        "setext\r",
        "---\r",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let found = headings(&document);

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].location.line, 2);
    assert_eq!(found[0].location.column, 4);
    assert_eq!(found[0].location.line_range, text_range(39, 50));
    assert!(found[0].suppressions.contains("skipped-level"));
    assert_eq!(found[1].location.line, 3);
    assert_eq!(found[1].location.line_range, text_range(51, 57));
}

#[test]
fn direct_blocks_follow_balanced_event_identity() {
    let source = concat!(
        "root paragraph\n\n",
        "<!-- transparent -->\n",
        "<div>visible</div>\n\n",
        "> quote\n>\n> ## nested heading\n\n",
        "- list item\n  - nested item\n\n",
        "```\ncode\n```\n\n",
        "---\n\n",
        "# section\n",
        "section paragraph\n\n",
        "> # nested boundary does not close\n> continuation\n\n",
        "after quote\n\n",
        "## child\n",
        "child paragraph\n",
    );
    let scanned = scan_preambles(source, MarkdownOptions::default());
    let root = scanned.root;
    let headings = scanned.headings;

    assert_eq!(
        root.iter().map(block_kind).collect::<Vec<_>>(),
        [
            BlockKind::Paragraph,
            BlockKind::Html,
            BlockKind::Quote,
            BlockKind::List,
            BlockKind::Code,
            BlockKind::Break,
        ]
    );
    assert_eq!(headings.len(), 2);
    assert_eq!(
        headings[0].iter().map(block_kind).collect::<Vec<_>>(),
        [BlockKind::Paragraph, BlockKind::Quote, BlockKind::Paragraph]
    );
    assert_eq!(
        headings[1].iter().map(block_kind).collect::<Vec<_>>(),
        [BlockKind::Paragraph]
    );
}

#[test]
fn tight_and_loose_first_paragraphs_have_equal_item_text() {
    let tight = parse_markdown(
        "- **A&amp;B** [link](target)\n- sibling\n",
        MarkdownOptions::default(),
    );
    let loose = parse_markdown(
        "- **A&amp;B** [link](target)\n\n- sibling\n",
        MarkdownOptions::default(),
    );
    let tight_text = lists(&tight)[0].items.first.text.as_ref();
    let loose_text = lists(&loose)[0].items.first.text.as_ref();

    assert_eq!(tight_text, loose_text);
    let text = tight_text.unwrap_or_else(|| unreachable!());
    assert_eq!(text.text, "A&B link");
    assert_eq!(text.diagnostic_text, "A&B link");
    assert_eq!(text.source_text, "**A&amp;B** [link](target)");

    let retained = parse_markdown(
        "- **A&amp;B** [link](target)\n",
        MarkdownOptions {
            strip_inline_markup: false,
        },
    );
    let retained = lists(&retained)[0]
        .items
        .first
        .text
        .as_ref()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(retained.text, "**A&B** [link](target)");
    assert_eq!(retained.diagnostic_text, "A&B link");
}

#[test]
fn nonparagraph_first_blocks_produce_no_item_text() {
    let source = concat!(
        "-\n",
        "- > quote first\n",
        "\n  later paragraph\n",
        "- ```\n  code\n  ```\n",
        "\n  later paragraph\n",
        "- - nested first\n",
        "\n  later paragraph\n",
        "- []()\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());
    let list = lists(&document)[0];
    let items: Vec<_> = list.items.iter().collect();

    assert_eq!(items.len(), 5);
    assert!(items[..4].iter().all(|item| item.text.is_none()));
    let present_empty = item_text(items[4]).unwrap_or_else(|| unreachable!());
    assert!(present_empty.text.is_empty());
    assert!(present_empty.diagnostic_text.is_empty());
    assert_eq!(present_empty.source_text, "[]()");
}

#[test]
fn nested_items_are_not_direct_items() {
    let document = parse_markdown(
        "- outer one\n  - nested one\n  - nested two\n- outer two\n",
        MarkdownOptions::default(),
    );
    let list = lists(&document)[0];
    let texts: Vec<_> = list
        .items
        .iter()
        .filter_map(item_text)
        .map(|text| text.diagnostic_text.as_str())
        .collect();

    assert_eq!(texts, ["outer one", "outer two"]);
    assert_eq!(list.items.rest.len(), 1);
}

#[test]
fn adjacent_lists_follow_parser_list_events() {
    let document = parse_markdown(
        concat!(
            "- first\n",
            "- second\n\n",
            "1. ordered\n",
            "2. continued\n\n",
            "<!-- transparent separator -->\n\n",
            "- final\n",
        ),
        MarkdownOptions::default(),
    );
    let lists = lists(&document);

    assert_eq!(lists.len(), 3);
    assert_eq!(lists[0].kind, ListKind::Bullet);
    assert_eq!(lists[0].items.iter().count(), 2);
    assert_eq!(lists[1].kind, ListKind::Ordered);
    assert_eq!(lists[1].items.iter().count(), 2);
    assert_eq!(lists[2].kind, ListKind::Bullet);
    assert_eq!(lists[2].items.iter().count(), 1);
}

#[test]
fn comment_only_html_uses_commonmark_comment_grammar() {
    for transparent in [
        "<!-- ordinary -->",
        " \t\r\n<!-->\x0c<!--->",
        "<!-- first --><!-- second -->",
        "<!-- multiline\ncomment -->\r\n",
    ] {
        assert!(is_comment_only_html(transparent), "{transparent:?}");
    }
    for visible in [
        "",
        "<!-- unterminated",
        "<!-- ok -->suffix",
        "<div><!-- nested --></div>",
        "<!-- first --><hr><!-- second -->",
        "<!-- first -->\x0b<!-- second -->",
        "\u{00a0}<!-- non-ASCII whitespace -->",
    ] {
        assert!(!is_comment_only_html(visible), "{visible:?}");
    }

    let source = "<!-- only -->\n\ntext <!-- inline --> stays\n";
    let root = scan_preambles(source, MarkdownOptions::default()).root;
    assert_eq!(root.len(), 1);
    assert_eq!(block_kind(&root[0]), BlockKind::Paragraph);

    let vertical_tab = "<!-- first -->\x0b<!-- second -->\n";
    let root = scan_preambles(vertical_tab, MarkdownOptions::default()).root;
    assert_eq!(root.len(), 1);
    assert_eq!(block_kind(&root[0]), BlockKind::Html);
}

#[test]
fn duplicate_normalized_reference_labels_do_not_create_or_merge_direct_blocks() {
    let source = "before\n\n[A  B]: /first\n[a b]: /duplicate\n\nafter\n";
    let scanned = scan_preambles(source, MarkdownOptions::default());
    let root = scanned.root;
    let definitions = scanned.reference_definitions;

    assert_eq!(
        root.iter().map(block_kind).collect::<Vec<_>>(),
        [BlockKind::Paragraph, BlockKind::Paragraph]
    );
    assert_eq!(definitions.len(), 1);
    assert!(definitions.contains_key("a b"));
    assert_eq!(block_location(&root[0]).range.start.0, 0);
    assert!([6, 7].contains(&block_location(&root[0]).range.end.0));
    assert_eq!(
        source
            .get(block_location(&root[0]).range.start.0..block_location(&root[0]).range.end.0)
            .map(trim_one_line_ending),
        Some("before")
    );
    let after_start = source.find("after").unwrap_or_else(|| unreachable!());
    assert_eq!(block_location(&root[1]).range.start.0, after_start);
    assert!([source.len() - 1, source.len()].contains(&block_location(&root[1]).range.end.0));
    assert_eq!(
        source
            .get(block_location(&root[1]).range.start.0..block_location(&root[1]).range.end.0)
            .map(trim_one_line_ending),
        Some("after")
    );
    for span in definitions.values() {
        assert!(root
            .iter()
            .all(|record| ranges_do_not_overlap(block_location(record), span)));
    }
}

#[test]
fn multiple_reference_definitions_in_one_region_do_not_create_or_merge_direct_blocks() {
    let source = "before\n\n[first]: /one\n[second]: /two\n[third]: /three\n\nafter\n";
    let scanned = scan_preambles(source, MarkdownOptions::default());
    let root = scanned.root;
    let definitions = scanned.reference_definitions;

    assert_eq!(definitions.len(), 3);
    assert_eq!(root.len(), 2);
    assert_eq!(block_location(&root[0]).range.start.0, 0);
    assert!([6, 7].contains(&block_location(&root[0]).range.end.0));
    assert_eq!(
        source
            .get(block_location(&root[0]).range.start.0..block_location(&root[0]).range.end.0)
            .map(trim_one_line_ending),
        Some("before")
    );
    let after_start = source.find("after").unwrap_or_else(|| unreachable!());
    assert_eq!(block_location(&root[1]).range.start.0, after_start);
    assert!([source.len() - 1, source.len()].contains(&block_location(&root[1]).range.end.0));
    assert_eq!(
        source
            .get(block_location(&root[1]).range.start.0..block_location(&root[1]).range.end.0)
            .map(trim_one_line_ending),
        Some("after")
    );
    for span in definitions.values() {
        assert!(root
            .iter()
            .all(|record| ranges_do_not_overlap(block_location(record), span)));
    }
}

fn ranges_do_not_overlap(left: &BlockLocation, right: &std::ops::Range<usize>) -> bool {
    left.range.end.0 <= right.start || right.end <= left.range.start.0
}

fn trim_one_line_ending(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix(['\r', '\n']))
        .unwrap_or(text)
}

#[test]
fn reference_definitions_inside_quotes_and_list_items_do_not_create_or_merge_direct_blocks() {
    let source = concat!(
        "> quoted\n>\n> [quote]: /q\n>\n> after\n\n",
        "- listed\n\n  [item]: /i\n\n  after\n\n",
        "tail\n",
    );
    let scanned = scan_preambles(source, MarkdownOptions::default());
    let root = scanned.root;
    let definitions = scanned.reference_definitions;

    assert_eq!(
        root.iter().map(block_kind).collect::<Vec<_>>(),
        [BlockKind::Quote, BlockKind::List, BlockKind::Paragraph]
    );
    assert_eq!(definitions.len(), 2);
}

#[test]
fn parser_ranges_pin_kind_specific_anchors_and_exclude_trailing_blanks() {
    let source = concat!(
        "<!-- å -->\r\n\r\n",
        "   paragraph\r\n\r\n",
        "  > quote\r\n\r\n",
        "    indented\r\n\r\n",
        "   12. ordered\r\n\r\n",
        "   ```rust\r\ncode\r\n   ```\r\n\r\n",
        "   <x-tag>\r\n\r\n",
        "  ***\r\n\r\n",
    );
    let root = scan_preambles(source, MarkdownOptions::default()).root;
    let anchors = [
        (BlockKind::Paragraph, "paragraph"),
        (BlockKind::Quote, ">"),
        (BlockKind::Code, "indented"),
        (BlockKind::List, "12."),
        (BlockKind::Code, "```"),
        (BlockKind::Html, "<x-tag>"),
        (BlockKind::Break, "***"),
    ];

    assert_eq!(root.len(), anchors.len());
    for (block, (kind, spelling)) in root.iter().zip(anchors) {
        let location = block_location(block);
        assert_eq!(block_kind(block), kind);
        assert_eq!(
            source.get(location.range.start.0..location.range.start.0 + spelling.len()),
            Some(spelling)
        );
        assert_eq!(
            location.line_range.start.0 + location.column as usize - 1,
            location.range.start.0
        );
        assert!(!source
            .get(location.range.start.0..location.range.end.0)
            .unwrap_or_default()
            .ends_with("\r\n\r\n"));
        let suppressions = match block {
            Block::Paragraph(block)
            | Block::Quote(block)
            | Block::Code(block)
            | Block::Html(block)
            | Block::Break(block) => &block.suppressions,
            Block::List(block) => &block.suppressions,
        };
        assert!(suppressions.0.is_empty());
    }

    let promoted = scan_preambles(
        "setext\r---\rsection paragraph\r",
        MarkdownOptions::default(),
    );
    assert!(promoted.root.is_empty());
    assert_eq!(promoted.headings.len(), 1);
    assert_eq!(promoted.headings[0].len(), 1);
    assert_eq!(block_kind(&promoted.headings[0][0]), BlockKind::Paragraph);
}

#[test]
fn malformed_and_deep_edge_forms_keep_prior_headings_and_direct_ownership() {
    let deep_quote = format!("{}nested\n\n# after deep quote\n", "> ".repeat(256));
    let cases = [
        (
            "---\r\nname: 界\r---\n# before fence\r\n```rust\r\n# swallowed\r\n",
            vec!["before fence"],
        ),
        (
            "# before comment\n\n<!-- incomplete --\n## swallowed by HTML\n",
            vec!["before comment"],
        ),
        (
            "999999999999999999999999999999999999. marker\n\n# after marker\n",
            vec!["after marker"],
        ),
        (
            "short setext\n--\n\nsetext one\r===\r\nsetext two\r\n---\n",
            vec!["short setext", "setext one", "setext two"],
        ),
        (deep_quote.as_str(), vec!["after deep quote"]),
    ];

    for (source, expected) in cases {
        let document = parse_markdown(source, MarkdownOptions::default());
        let actual: Vec<_> = headings(&document)
            .into_iter()
            .map(|heading| heading.diagnostic_text.as_str())
            .collect();
        assert_eq!(actual, expected, "{source:?}");
    }

    let nested = parse_markdown(&deep_quote, MarkdownOptions::default());
    assert_eq!(nested.preamble.len(), 1);
    let block = nested
        .preamble
        .as_slice()
        .first()
        .unwrap_or_else(|| unreachable!());
    assert_eq!(block_kind(block), BlockKind::Quote);
}

#[test]
fn line_index_treats_crlf_as_one_ending_and_cr_as_an_ending() {
    let source = "a\r\nb\rc\nd";
    let lines = LineIndex::new(source);
    let actual: Vec<_> = (1..=lines.line_count())
        .map(|line| lines.line_text(source, line))
        .collect();

    assert_eq!(actual, [Some("a"), Some("b"), Some("c"), Some("d")]);
    assert_eq!(lines.line_number(3), 2);
    assert_eq!(lines.line_number(5), 3);
    assert_eq!(lines.line_number(7), 4);
}

#[test]
fn ignores_suppression_spelling_near_misses_and_code() {
    let source = concat!(
        "```html\n<!-- outlint-disable-file skipped-level -->\n```\n",
        "<!-- outlint-disable-filed not-allowed -->\n",
        "<!-- outlint-disable -->\n",
        "# heading\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    assert!(document.file_suppressions.0.is_empty());
    assert!(document.sections[0].heading.suppressions.0.is_empty());
}

#[test]
fn parses_and_masks_yaml_frontmatter_before_heading_scanning() {
    let source = concat!(
        "---\n",
        "title: metadata, not a setext heading\n",
        "draft: false\n",
        "tags: [one, two]\n",
        "---\n",
        "# Document title\n",
    );
    let document = parse_markdown(source, MarkdownOptions::default());

    let DocumentFrontmatter::Mapping {
        value, location, ..
    } = &document.frontmatter
    else {
        panic!("expected parsed frontmatter")
    };
    assert_eq!(value.get("draft"), Some(&serde_json::Value::Bool(false)));
    assert_eq!(location.start_line, 1);
    assert_eq!(location.end_line, 5);
    assert_eq!(headings(&document).len(), 1);
    assert_eq!(headings(&document)[0].diagnostic_text, "Document title");
}
