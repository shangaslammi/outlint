use crate::markdown::lines::{text_range, LineIndex};
use crate::markdown::{
    parse_markdown, Document, DocumentFrontmatter, Heading, MarkdownOptions, Section,
};
use crate::HeaderLevel;
use pulldown_cmark::{Tag, TagEnd};

use super::super::body::{is_comment_only_html, scan_preambles, BlockKind, FrameStack};

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
        root.iter().map(|record| record.kind).collect::<Vec<_>>(),
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
        headings[0]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [BlockKind::Paragraph, BlockKind::Quote, BlockKind::Paragraph]
    );
    assert_eq!(
        headings[1]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [BlockKind::Paragraph]
    );
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
    assert_eq!(root[0].kind, BlockKind::Paragraph);

    let vertical_tab = "<!-- first -->\x0b<!-- second -->\n";
    let root = scan_preambles(vertical_tab, MarkdownOptions::default()).root;
    assert_eq!(root.len(), 1);
    assert_eq!(root[0].kind, BlockKind::Html);
}

#[test]
fn duplicate_normalized_reference_labels_do_not_create_or_merge_direct_blocks() {
    let source = "before\n\n[A  B]: /first\n[a b]: /duplicate\n\nafter\n";
    let scanned = scan_preambles(source, MarkdownOptions::default());
    let root = scanned.root;
    let definitions = scanned.reference_definitions;

    assert_eq!(
        root.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [BlockKind::Paragraph, BlockKind::Paragraph]
    );
    assert_eq!(definitions.len(), 1);
    assert!(definitions.contains_key("a b"));
    assert_eq!(root[0].range.start, 0);
    assert!([6, 7].contains(&root[0].range.end));
    assert_eq!(
        source.get(root[0].range.clone()).map(trim_one_line_ending),
        Some("before")
    );
    let after_start = source.find("after").unwrap_or_else(|| unreachable!());
    assert_eq!(root[1].range.start, after_start);
    assert!([source.len() - 1, source.len()].contains(&root[1].range.end));
    assert_eq!(
        source.get(root[1].range.clone()).map(trim_one_line_ending),
        Some("after")
    );
    for span in definitions.values() {
        assert!(root
            .iter()
            .all(|record| ranges_do_not_overlap(&record.range, span)));
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
    assert_eq!(root[0].range.start, 0);
    assert!([6, 7].contains(&root[0].range.end));
    assert_eq!(
        source.get(root[0].range.clone()).map(trim_one_line_ending),
        Some("before")
    );
    let after_start = source.find("after").unwrap_or_else(|| unreachable!());
    assert_eq!(root[1].range.start, after_start);
    assert!([source.len() - 1, source.len()].contains(&root[1].range.end));
    assert_eq!(
        source.get(root[1].range.clone()).map(trim_one_line_ending),
        Some("after")
    );
    for span in definitions.values() {
        assert!(root
            .iter()
            .all(|record| ranges_do_not_overlap(&record.range, span)));
    }
}

fn ranges_do_not_overlap(left: &std::ops::Range<usize>, right: &std::ops::Range<usize>) -> bool {
    left.end <= right.start || right.end <= left.start
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
        root.iter().map(|record| record.kind).collect::<Vec<_>>(),
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
    for (record, (kind, spelling)) in root.iter().zip(anchors) {
        assert_eq!(record.kind, kind);
        assert_eq!(
            source.get(record.range.start..record.range.start + spelling.len()),
            Some(spelling)
        );
        assert_eq!(
            record.line_range.start + record.column as usize - 1,
            record.range.start
        );
        assert!(!source
            .get(record.range.clone())
            .unwrap_or_default()
            .ends_with("\r\n\r\n"));
        assert!(record.suppressions.0.is_empty());
    }

    let promoted = scan_preambles(
        "setext\r---\rsection paragraph\r",
        MarkdownOptions::default(),
    );
    assert!(promoted.root.is_empty());
    assert_eq!(promoted.headings.len(), 1);
    assert_eq!(promoted.headings[0].len(), 1);
    assert_eq!(promoted.headings[0][0].kind, BlockKind::Paragraph);
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
