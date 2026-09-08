use proptest::prelude::*;

use crate::markdown::frontmatter::yaml::push_pointer_token;
use crate::markdown::lines::LineIndex;
use crate::markdown::{
    parse_markdown, Block, BlockLocation, DocumentFrontmatter, FrontmatterAnchors,
    FrontmatterLocation, Heading, ItemLocation, MarkdownOptions, Section,
};
use crate::{HeaderLevel, TextRange};

use super::super::body::scan_preambles;
use super::assert_distinct_anchors;

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

fn block_kind(block: &Block) -> crate::markdown::BlockKind {
    match block {
        Block::Paragraph(_) => crate::markdown::BlockKind::Paragraph,
        Block::List(_) => crate::markdown::BlockKind::List,
        Block::Quote(_) => crate::markdown::BlockKind::Quote,
        Block::Code(_) => crate::markdown::BlockKind::Code,
        Block::Html(_) => crate::markdown::BlockKind::Html,
        Block::Break(_) => crate::markdown::BlockKind::Break,
    }
}

fn assert_valid_range(source: &str, range: TextRange) {
    assert!(range.start <= range.end);
    assert!(range.end.0 <= source.len());
    assert!(source.is_char_boundary(range.start.0));
    assert!(source.is_char_boundary(range.end.0));
}

fn assert_valid_section_ranges(source: &str, sections: &[Section]) {
    for section in sections {
        assert_valid_range(source, section.heading.location.range);
        assert_valid_range(source, section.heading.location.line_range);
        assert!(section.heading.location.line >= 1);
        assert!(section.heading.location.column >= 1);
        assert_valid_section_ranges(source, &section.children);
    }
}

fn assert_valid_preamble_ranges(source: &str, blocks: &[Block]) {
    let lines = LineIndex::new(source);
    let mut prior_end = None;
    for block in blocks {
        let location = block_location(block);
        assert_valid_range(source, location.range);
        assert_valid_range(source, location.line_range);
        assert!(location.line >= 1);
        assert!(location.column >= 1);
        assert_eq!(
            lines.line_start(location.line as usize) + location.column as usize - 1,
            location.range.start.0
        );
        if let Some(prior_end) = prior_end {
            assert!(prior_end <= location.range.start);
        }
        prior_end = Some(location.range.end);
        assert_no_trailing_blank_line(source, location.range);
        if let Block::List(list) = block {
            assert!(list.items.iter().next().is_some());
            let mut prior_item_end = None;
            for item in list.items.iter() {
                assert_valid_range(source, item.location.range);
                assert_valid_range(source, item.location.line_range);
                assert!(location.range.start <= item.location.range.start);
                assert!(item.location.range.end <= location.range.end);
                assert_item_anchor(source, &lines, &item.location);
                if let Some(prior_item_end) = prior_item_end {
                    assert!(prior_item_end <= item.location.range.start);
                }
                prior_item_end = Some(item.location.range.end);
                assert_no_trailing_blank_line(source, item.location.range);
            }
        }
    }
}

fn assert_item_anchor(source: &str, lines: &LineIndex, location: &ItemLocation) {
    assert!(location.line >= 1);
    assert!(location.column >= 1);
    assert_eq!(
        lines.line_start(location.line as usize) + location.column as usize - 1,
        location.range.start.0
    );
    assert!(source.is_char_boundary(location.range.start.0));
}

/// A parser span may include its terminating line ending, but never a blank
/// line after the last content line (§1.7).
fn assert_no_trailing_blank_line(source: &str, range: TextRange) {
    let text = source
        .get(range.start.0..range.end.0)
        .unwrap_or_else(|| unreachable!());
    let without_terminator = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix(['\r', '\n']))
        .unwrap_or(text);
    let final_line = without_terminator
        .rsplit(['\r', '\n'])
        .next()
        .unwrap_or_default();
    assert!(
        final_line.bytes().any(|byte| !matches!(byte, b' ' | b'\t')),
        "range retained a trailing blank line: {range:?}"
    );
}

fn assert_document_block_contract(source: &str, sections: &[Section], root: &[Block]) {
    assert_valid_preamble_ranges(source, root);
    for section in sections {
        assert_valid_preamble_ranges(source, section.preamble.as_slice());
        assert_document_block_contract(source, &section.children, &[]);
    }
}

fn flatten_sections<'a>(sections: &'a [Section], output: &mut Vec<&'a Section>) {
    for section in sections {
        output.push(section);
        flatten_sections(&section.children, output);
    }
}

fn heading_projection(sections: &[Section]) -> Vec<(HeaderLevel, String)> {
    fn visit(sections: &[Section], output: &mut Vec<(HeaderLevel, String)>) {
        for Section {
            heading: Heading { level, text, .. },
            children,
            ..
        } in sections
        {
            output.push((*level, text.clone()));
            visit(children, output);
        }
    }

    let mut output = Vec::new();
    visit(sections, &mut output);
    output
}

fn assert_valid_anchors(
    source: &str,
    location: &FrontmatterLocation,
    anchors: &FrontmatterAnchors,
) {
    let lines = LineIndex::new(source);
    for (pointer, anchor) in &anchors.0 {
        assert!(
            (2..location.end_line).contains(&anchor.line),
            "{pointer} left the block: {anchor:?}"
        );
        let text = lines
            .line_text(source, anchor.line as usize)
            .unwrap_or_else(|| panic!("{pointer} names a line the document lacks"));
        let column = anchor.column as usize - 1;
        assert!(
            column <= text.len(),
            "{pointer} overruns its line: {anchor:?}"
        );
        assert!(
            text.is_char_boundary(column),
            "{pointer} splits a character: {anchor:?}"
        );
    }
    assert_distinct_anchors(source, anchors);
}

/// Every entry that must hold a position holds one, counted.
///
/// The invariants above bind only the anchors that are there, so recording
/// none at all would satisfy every one of them. This is the floor under
/// them. An entry is required to hold a position when its spelling must
/// have had a character for the parser to mark: a member whose key is not
/// all line breaks, and an element whose value cannot have come from a
/// textless spelling. A null element is exempt, since `-` and `null` yield
/// the same value, and so is an all-break string, since `- >-` and `- "\n"`
/// do — under the narrowed rule several of those exempt spellings do keep
/// an anchor, which the floor permits without requiring.
///
/// The count returned is how many entries this document required, and the
/// yield report is what keeps the exemptions from swallowing the floor: it
/// counts the entries required across a run, which an implementation that
/// dropped every anchor would drive to zero.
fn assert_written_entries_keep_anchors(
    source: &str,
    value: &serde_json::Map<String, serde_json::Value>,
    anchors: &FrontmatterAnchors,
) -> usize {
    assert_written_members_keep_anchors(source, value, &mut String::new(), anchors)
}

fn assert_written_members_keep_anchors(
    source: &str,
    members: &serde_json::Map<String, serde_json::Value>,
    pointer: &mut String,
    anchors: &FrontmatterAnchors,
) -> usize {
    let mut required = 0;
    for (key, member) in members {
        let restore = pointer.len();
        push_pointer_token(pointer, key);
        if !text_may_be_textless(key) {
            required += 1;
            assert_anchor_kept(source, pointer, anchors);
        }
        required += assert_written_values_keep_anchors(source, member, pointer, anchors);
        pointer.truncate(restore);
    }
    required
}

fn assert_written_values_keep_anchors(
    source: &str,
    value: &serde_json::Value,
    pointer: &mut String,
    anchors: &FrontmatterAnchors,
) -> usize {
    match value {
        serde_json::Value::Object(members) => {
            assert_written_members_keep_anchors(source, members, pointer, anchors)
        }
        serde_json::Value::Array(elements) => {
            let mut required = 0;
            for (index, element) in elements.iter().enumerate() {
                let restore = pointer.len();
                pointer.push('/');
                pointer.push_str(&index.to_string());
                if !value_may_be_textless(element) {
                    required += 1;
                    assert_anchor_kept(source, pointer, anchors);
                }
                required += assert_written_values_keep_anchors(source, element, pointer, anchors);
                pointer.truncate(restore);
            }
            required
        }
        _ => 0,
    }
}

fn assert_anchor_kept(source: &str, pointer: &str, anchors: &FrontmatterAnchors) {
    assert!(
        anchors.get(pointer).is_some(),
        "{pointer} is written but kept no anchor in {source:?}"
    );
}

/// Whether a converted value could have been spelled with no text at all.
fn value_may_be_textless(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(text) => text_may_be_textless(text),
        _ => false,
    }
}

/// Whether text could have come from a spelling with no character in it.
///
/// Written out here rather than taken from
/// [`is_textless`](crate::markdown::frontmatter::yaml::is_textless) on purpose: a
/// floor that called the rule it is holding up would widen along with it,
/// and a rule that discarded more positions than line breaks force would
/// pass unnoticed.
fn text_may_be_textless(text: &str) -> bool {
    text.chars().all(|character| character == '\n')
}

/// The element spellings a generated block sequence draws from.
///
/// The first seven are textless in one form or another — the class the
/// anchor floor exempts, of which only the empty block scalars actually
/// borrow a later entry's marker now; the rest are written and must keep
/// a position of their own, `- " "` among them, since the rule turns on
/// line breaks alone and a space is a character like any other. A spelling
/// may span lines, so it carries its own continuation, indented past the
/// `-` that opens it.
///
/// The mappings under a quoted empty key are here because the element they
/// open anchors at that quote — the mapping-start marker sits on the first
/// key's first character — and a corpus that cannot spell the shape cannot
/// witness it at all. Both syntaxes are drawn: the block form starts at
/// the quote, the flow form at its `{`.
const ARBITRARY_ELEMENTS: &[&str] = &[
    "-",
    "- \"\"",
    "- ''",
    "- >-",
    "- |",
    "- |+\n",
    "- |+\n\n",
    "- null",
    "- ~",
    "- 1",
    "- ok",
    "- \" \"",
    "- >-\n    text",
    "- |\n    text",
    "- key: 1",
    "- [1, 2]",
    "- {p: 1}",
    "- \"\": 1",
    "- '': 1\n    next: 2",
    "- {\"\": 1}",
    "- {'': 1, next: 2}",
];

/// The prefix of [`ARBITRARY_ELEMENTS`] whose elements have no text.
const ARBITRARY_TEXTLESS_ELEMENTS: usize = 7;

/// The suffix of [`ARBITRARY_ELEMENTS`] that are mappings under a quoted
/// empty key, the shape anchored at the key's own opening quote.
const ARBITRARY_EMPTY_KEY_ELEMENTS: usize = 4;

/// Whether a document holds one of these spellings as a whole entry.
///
/// Naive containment overcounts, because a textless spelling is a prefix of
/// a written one: `- >-` opens `- >-\n    text` too, so a document holding
/// only the written form would be counted as holding a textless element. A
/// match therefore counts only when nothing continues the spelling — no
/// line indented past the two columns the entry itself sits at, and no
/// `  : ` line giving an explicit key its value.
fn holds_spelling(source: &str, spellings: &[&str]) -> bool {
    spellings.iter().any(|spelling| {
        let written = format!("\n  {}\n", spelling.trim_end());
        source.match_indices(&written).any(|(index, matched)| {
            let rest = &source[index + matched.len()..];
            !rest.starts_with("   ") && !rest.starts_with("  : ")
        })
    })
}

/// The key spellings a generated nested mapping draws its first member
/// from, indented two columns in.
///
/// The first five are textless keys, which YAML admits only through the
/// explicit `? ` form and which borrow the following member's marker; the
/// rest are written and must keep a position of their own. Each spelling
/// carries its own continuation lines, and the mapping it opens is closed
/// off by a written member, so a borrowed marker always has a neighbour to
/// collide with.
const ARBITRARY_KEYS: &[&str] = &[
    "? >-",
    "? |",
    "? |+\n",
    "? \"\"",
    "? ''",
    "? >-\n    text",
    "? |\n    text",
    "? \" \"",
    "? plain",
    "? plain\n  : 1",
    "? multi\n    line\n  : 1",
    "plain: 1",
    "\"quoted\": 1",
    "'single': 1",
];

/// The prefix of [`ARBITRARY_KEYS`] whose keys have no text.
const ARBITRARY_TEXTLESS_KEYS: usize = 5;

/// A frontmatter block of arbitrary entries, some of which parse.
///
/// `any::<String>()` cannot reach a parsed mapping: its default strategy
/// excludes control characters, so the generated text never contains the
/// newline a closing `---` needs. Anchors need a generator shaped like a
/// block to exercise them at all.
///
/// Keys carry their index so that entries cannot collide, since a duplicate
/// key is rejected before any anchor is recorded and would spend the case.
/// Indentation is skewed to zero for the same reason: a top-level entry
/// indented past the first one is invalid YAML, and every entry of a case
/// has to be well placed for the case to reach a mapping at all.
fn arbitrary_frontmatter_document() -> impl Strategy<Value = String> {
    let indent = prop_oneof![9 => Just(0usize), 1 => 1usize..3];
    let body = prop_oneof![
        // `key: value`, plain or wrapped in flow brackets.
        2 => (proptest::bool::ANY, "([a-z0-9\u{00e4}\u{00f6} ]{0,8}|[a-z0-9\u{00e4}\u{00f6}, ]{0,8}|(\r|[ ]|.){0,10})")
            .prop_map(|(flow, value)| if flow { format!(" [{value}]") } else { format!(" {value}") }),
        // A block sequence, whose elements are named by position alone.
        1 => proptest::collection::vec(0..ARBITRARY_ELEMENTS.len(), 1..5)
            .prop_map(|elements| {
                let mut text = String::new();
                for element in elements {
                    text.push_str("\n  ");
                    text.push_str(ARBITRARY_ELEMENTS[element]);
                }
                text
            }),
        // A nested mapping, whose members are named by their keys. Only
        // one drawn key per mapping: the textless spellings all parse to
        // the same key, and a duplicate would spend the case.
        1 => (0..ARBITRARY_KEYS.len()).prop_map(|key| {
            format!("\n  {}\n  next: 2", ARBITRARY_KEYS[key])
        }),
    ];
    proptest::collection::vec(("[a-z\u{00e0}-\u{00ff}]{1,3}", indent, body), 1..6).prop_map(
        |entries| {
            let mut text = String::new();
            for (index, (key, indent, body)) in entries.into_iter().enumerate() {
                text.push_str(&" ".repeat(indent));
                text.push_str(&key);
                text.push_str(&index.to_string());
                text.push(':');
                text.push_str(&body);
                text.push('\n');
            }
            format!("---\n{text}---\n\n# Title\n")
        },
    )
}

proptest! {
    #[test]
    fn public_preamble_tree_preserves_parentage_and_ranges(
        root_blocks in 0usize..4,
        sections in proptest::collection::vec((1usize..5, "[a-z]{1,8}"), 1..16),
    ) {
        let mut source = String::new();
        for index in 0..root_blocks {
            source.push_str(&format!("root-{index}\n\n"));
        }
        for (index, (level, name)) in sections.iter().enumerate() {
            source.push_str(&format!("{} {name}-{index}\n\n- item-{index}\n\n", "#".repeat(*level)));
        }

        let document = parse_markdown(&source, MarkdownOptions::default());
        prop_assert_eq!(document.preamble.len(), root_blocks);
        assert_valid_preamble_ranges(&source, document.preamble.as_slice());

        let mut flattened = Vec::new();
        flatten_sections(&document.sections, &mut flattened);
        prop_assert_eq!(flattened.len(), sections.len());
        for (index, section) in flattened.into_iter().enumerate() {
            prop_assert_eq!(section.preamble.len(), 1);
            let block = section.preamble.as_slice().first().unwrap_or_else(|| unreachable!());
            let Block::List(list) = block else {
                prop_assert!(false, "section preamble did not retain its list");
                continue;
            };
            let text = list.items.first.text.as_ref().unwrap_or_else(|| unreachable!());
            prop_assert_eq!(text.diagnostic_text.as_str(), format!("item-{index}"));
            prop_assert!(section.heading.location.range.end <= list.location.range.start);
            assert_valid_preamble_ranges(&source, section.preamble.as_slice());
        }
    }

    #[test]
    fn parser_spans_and_anchors_are_in_bounds_utf8_boundaries(
        prefix in "[é界]{0,4}",
        indent in 0usize..4,
        content in "[a-zé界]{1,16}",
        ending in prop_oneof![Just("\n"), Just("\r\n"), Just("\r")],
    ) {
        let source = format!(
            "<!-- {prefix} -->{ending}{ending}{}{content}{ending}{ending}# next{ending}",
            " ".repeat(indent),
        );
        let scanned = scan_preambles(&source, MarkdownOptions::default());
        let root = scanned.root;
        let headings = scanned.headings;
        prop_assert_eq!(root.len(), 1);
        prop_assert_eq!(headings.len(), 1);
        let record = block_location(&root[0]);
        let expected_start = "<!--  -->".len() + prefix.len() + ending.len() * 2 + indent;
        prop_assert_eq!(record.range.start.0, expected_start);
        prop_assert!(record.range.start <= record.range.end);
        prop_assert!(record.range.end.0 <= expected_start + content.len() + ending.len());
        prop_assert!(source.is_char_boundary(record.range.start.0));
        prop_assert!(source.is_char_boundary(record.range.end.0));
        prop_assert!(record.line_range.start <= record.range.start);
        prop_assert!(record.range.start <= record.line_range.end);
        prop_assert_eq!(record.column as usize, indent + 1);
    }

    #[test]
    fn frame_scan_preserves_heading_projection(
        cases in proptest::collection::vec(
            (0usize..5, 1usize..8, "[a-z]{1,8}", any::<bool>(), any::<bool>()),
            0..40,
        ),
    ) {
        let mut source = String::new();
        let mut expected = Vec::new();
        for (indent, level, text, setext, nested) in cases {
            let spaces = " ".repeat(indent);
            if nested {
                if setext {
                    let underline = if level % 2 == 0 { '-' } else { '=' };
                    source.push_str(&format!("> {text}\n> {}\n\n", underline.to_string().repeat(3)));
                } else {
                    source.push_str(&format!("> {} {text}\n\n", "#".repeat(level)));
                }
                continue;
            }

            if setext {
                let (underline, expected_level) = if level % 2 == 0 {
                    ('-', HeaderLevel::H2)
                } else {
                    ('=', HeaderLevel::H1)
                };
                source.push_str(&format!(
                    "{spaces}{text}\n{spaces}{}\n\n",
                    underline.to_string().repeat(3),
                ));
                if indent <= 3 {
                    expected.push((expected_level, text));
                }
            } else {
                source.push_str(&format!("{spaces}{} {text}\n\n", "#".repeat(level)));
                if indent <= 3 && level <= 6 {
                    let expected_level = match level {
                        1 => HeaderLevel::H1,
                        2 => HeaderLevel::H2,
                        3 => HeaderLevel::H3,
                        4 => HeaderLevel::H4,
                        5 => HeaderLevel::H5,
                        6 => HeaderLevel::H6,
                        _ => continue,
                    };
                    expected.push((expected_level, text));
                }
            }
        }

        let document = parse_markdown(&source, MarkdownOptions::default());
        prop_assert_eq!(heading_projection(&document.sections), expected);
    }

    #[test]
    fn malformed_mixed_markdown_never_panics(
        depth in 1usize..96,
        malformed in 0u8..4,
        huge_marker in "[0-9]{20,80}",
        inline_units in 0usize..256,
        endings in proptest::collection::vec(0u8..3, 4..24),
    ) {
        let ending = |index: usize| match endings.get(index % endings.len()).copied() {
            Some(0) => "\n",
            Some(1) => "\r\n",
            _ => "\r",
        };
        let mut source = format!(
            "---{0}name: 界{1}---{2}{3}# stable{4}{5}",
            ending(0), ending(1), ending(2), ending(3), ending(4), ending(5),
        );
        source.push_str(&"> ".repeat(depth));
        source.push_str("- nested **é**");
        source.push_str(ending(6));
        source.push_str(" \t");
        source.push_str(ending(7));
        source.push_str("near setext");
        source.push_str(ending(8));
        source.push_str("--");
        source.push_str(ending(9));
        source.push_str(ending(10));
        source.push_str("[A  B]: /first");
        source.push_str(ending(11));
        source.push_str("[a b]: /duplicate");
        source.push_str(ending(12));
        source.push_str(ending(13));
        source.push_str(&"é*_`<&amp;".repeat(inline_units));
        source.push_str(ending(14));
        match malformed {
            0 => source.push_str("```rust\r\n# unterminated fence"),
            1 => source.push_str("<!-- unterminated comment --"),
            2 => source.push_str(&format!("{huge_marker}. enormous marker\n  > child")),
            _ => source.push_str("- item\n  > quote\n    ~~~~\n    unclosed"),
        }

        let document = parse_markdown(&source, MarkdownOptions::default());
        assert_valid_section_ranges(&source, &document.sections);
        assert_document_block_contract(
            &source,
            &document.sections,
            document.preamble.as_slice(),
        );
        prop_assert_eq!(
            heading_projection(&document.sections),
            [
                (HeaderLevel::H1, "stable".to_owned()),
                (HeaderLevel::H2, "near setext".to_owned()),
            ]
        );
    }

    #[test]
    fn mixed_newlines_and_unicode_preserve_anchor_contract(
        paragraph_tail in "[a-zé界]{1,24}",
        item_tail in "[a-zé界]{1,24}",
        indent in 0usize..4,
    ) {
        let paragraph = format!("é{paragraph_tail}");
        let item = format!("界{item_tail}");
        let source = format!(
            "---\r\nname: 界\r---\n\n<!-- transparent -->\r{}{paragraph}\r\n\r  > quote 界\n\r\n   12. {item}\r\r# 標題\r\nsetext é\r---\r\n",
            " ".repeat(indent),
        );
        let document = parse_markdown(&source, MarkdownOptions::default());
        let root = document.preamble.as_slice();
        prop_assert_eq!(
            root.iter().map(block_kind).collect::<Vec<_>>(),
            [
                crate::markdown::BlockKind::Paragraph,
                crate::markdown::BlockKind::Quote,
                crate::markdown::BlockKind::List,
            ]
        );
        let paragraph_start = source.find(&paragraph).unwrap_or_else(|| unreachable!());
        let quote_start = source.find("> quote").unwrap_or_else(|| unreachable!());
        let list_start = source.find("12.").unwrap_or_else(|| unreachable!());
        let paragraph_bound = paragraph_start + paragraph.len() + "\r\n".len();
        let quote_bound = quote_start + "> quote 界\n".len();
        let list_bound = list_start + "12. ".len() + item.len() + "\r".len();
        for (block, (start, end_bound)) in root.iter().zip([
            (paragraph_start, paragraph_bound),
            (quote_start, quote_bound),
            (list_start, list_bound),
        ]) {
            prop_assert_eq!(block_location(block).range.start.0, start);
            prop_assert!(block_location(block).range.end.0 <= end_bound);
        }
        let Block::List(list) = root.last().unwrap_or_else(|| unreachable!()) else {
            prop_assert!(false, "ordered marker did not form a list");
            return Ok(());
        };
        prop_assert_eq!(list.items.first.location.range.start.0, list_start);
        prop_assert_eq!(list.items.first.text.as_ref().map(|text| text.diagnostic_text.as_str()), Some(item.as_str()));
        prop_assert_eq!(
            heading_projection(&document.sections),
            [
                (HeaderLevel::H1, "標題".to_owned()),
                (HeaderLevel::H2, "setext é".to_owned()),
            ]
        );
        assert_valid_section_ranges(&source, &document.sections);
        assert_document_block_contract(&source, &document.sections, root);
    }

    #[test]
    fn nested_container_ranges_preserve_direct_parentage(
        depth in 1usize..64,
        item_count in 1usize..8,
        malformed_tail in any::<bool>(),
    ) {
        let mut source = String::from("before\n\n");
        let quote_start = source.len();
        source.push_str(&"> ".repeat(depth));
        source.push_str("deep quote\n\n");
        let list_start = source.len();
        for index in 0..item_count {
            source.push_str(&format!("- item-{index}\n  > nested-{index}\n"));
        }
        if malformed_tail {
            source.push_str("  <!-- unterminated\n");
        }
        source.push_str("\nafter\n");

        let document = parse_markdown(&source, MarkdownOptions::default());
        let root = document.preamble.as_slice();
        prop_assert_eq!(
            root.iter().map(block_kind).collect::<Vec<_>>(),
            [
                crate::markdown::BlockKind::Paragraph,
                crate::markdown::BlockKind::Quote,
                crate::markdown::BlockKind::List,
                crate::markdown::BlockKind::Paragraph,
            ]
        );
        let [_, quote, list, _] = root else {
            prop_assert!(false, "expected four direct root blocks");
            return Ok(());
        };
        prop_assert_eq!(block_location(quote).range.start.0, quote_start);
        prop_assert_eq!(block_location(list).range.start.0, list_start);
        let Block::List(list) = list else {
            prop_assert!(false, "direct list lost its event identity");
            return Ok(());
        };
        prop_assert_eq!(list.items.iter().count(), item_count);
        for (index, item) in list.items.iter().enumerate() {
            prop_assert_eq!(
                item.text.as_ref().map(|text| text.diagnostic_text.clone()),
                Some(format!("item-{index}")),
            );
        }
        assert_document_block_contract(&source, &document.sections, root);
    }

    #[test]
    fn transparency_never_merges_event_defined_siblings(
        neighborhoods in 1usize..8,
        comment_body in "[a-z é界]{0,24}",
        comment_case in 0u8..10,
    ) {
        // pulldown-cmark 0.13.4 scanners.rs:1448-1468 implements the
        // CommonMark 0.31.2 rule by scanning only for the closing `-->`.
        let (comment, visible_kind) = match comment_case {
            0 => (format!("<!-- {comment_body} -->"), None),
            1 => ("<!-->".to_owned(), None),
            2 => ("<!--->".to_owned(), None),
            3 => ("<!-- <tag> > -->".to_owned(), None),
            4 => ("<!-- a -- b -->".to_owned(), None),
            5 => ("<!-- -> x -->".to_owned(), None),
            6 => ("<!---> trailing".to_owned(), Some(crate::markdown::BlockKind::Html)),
            7 => ("<!-- ok -->suffix".to_owned(), Some(crate::markdown::BlockKind::Html)),
            8 => ("<!-- first --><hr><!-- second -->".to_owned(), Some(crate::markdown::BlockKind::Html)),
            _ => ("<!- unclosed".to_owned(), Some(crate::markdown::BlockKind::Paragraph)),
        };
        let mut source = String::new();
        let mut expected_starts = Vec::new();
        let mut expected_kinds = Vec::new();
        for index in 0..neighborhoods {
            expected_starts.push(source.len());
            expected_kinds.push(crate::markdown::BlockKind::Paragraph);
            source.push_str(&format!("visible-{index}\n\n"));
            if let Some(kind) = visible_kind {
                expected_starts.push(source.len());
                expected_kinds.push(kind);
            }
            source.push_str(&comment);
            source.push_str("\n\n");
            source.push_str("[A  B]: /first\n[a b]: /duplicate\n\n");
            expected_starts.push(source.len());
            expected_kinds.push(crate::markdown::BlockKind::Html);
            source.push_str(&format!("<div>html-{index}</div>\n\n"));
            expected_starts.push(source.len());
            expected_kinds.push(crate::markdown::BlockKind::Quote);
            source.push_str("> quoted-before\n>\n> [Q  A]: /quote\n> [q a]: /duplicate\n>\n> quoted-after\n\n");
            expected_starts.push(source.len());
            expected_kinds.push(crate::markdown::BlockKind::List);
            source.push_str("- listed-before\n\n  [I  A]: /item\n  [i a]: /duplicate\n\n  listed-after\n\n");
        }
        let list_start = source.len();
        expected_starts.push(list_start);
        expected_kinds.push(crate::markdown::BlockKind::List);
        source.push_str("1. **tight**\n");

        let scanned = scan_preambles(&source, MarkdownOptions::default());
        prop_assert_eq!(
            scanned.root.iter().map(block_kind).collect::<Vec<_>>(),
            expected_kinds
        );
        prop_assert_eq!(
            scanned.root.iter().map(|block| block_location(block).range.start.0).collect::<Vec<_>>(),
            expected_starts
        );
        prop_assert_eq!(scanned.reference_definitions.len(), 3);
        let Block::List(list) = scanned.root.last().unwrap_or_else(|| unreachable!()) else {
            prop_assert!(false, "tight list was not retained");
            return Ok(());
        };
        prop_assert_eq!(list.location.range.start.0, list_start);
        prop_assert_eq!(
            list.items.first.text.as_ref().map(|text| text.diagnostic_text.as_str()),
            Some("tight")
        );
        assert_valid_preamble_ranges(&source, &scanned.root);
    }

    #[test]
    fn arbitrary_utf8_input_is_total_and_offsets_are_valid(source in any::<String>()) {
        let document = parse_markdown(&source, MarkdownOptions::default());
        assert_valid_section_ranges(&source, &document.sections);
        // Anchors are not asserted here: this strategy never emits a
        // newline, so no input of it reaches a parsed mapping.
        match document.frontmatter {
            DocumentFrontmatter::Absent => {}
            DocumentFrontmatter::Mapping { location, .. }
            | DocumentFrontmatter::Invalid { location, .. } => {
                assert_valid_range(&source, location.range);
                prop_assert!(location.start_line >= 1);
                prop_assert!(location.end_line >= location.start_line);
            }
        }
    }

    #[test]
    fn frontmatter_anchors_stay_within_their_own_line(
        source in arbitrary_frontmatter_document(),
    ) {
        let document = parse_markdown(&source, MarkdownOptions::default());
        if let DocumentFrontmatter::Mapping { location, value, anchors } = &document.frontmatter {
            assert_valid_anchors(&source, location, anchors);
            assert_written_entries_keep_anchors(&source, value, anchors);
        }
    }
}

#[test]
fn arbitrary_frontmatter_documents_reach_textless_entries() {
    // A generator that cannot reach the shape under test leaves a dead
    // property that passes forever. This one has to reach a parsed mapping
    // holding a block sequence, a nested mapping, and a textless entry of
    // either kind, often enough that the anchor invariants are actually
    // being exercised — and it has to leave written entries behind for the
    // retention floor to hold up.
    use proptest::{strategy::ValueTree, test_runner::TestRunner};

    const SAMPLES: usize = 512;
    let strategy = arbitrary_frontmatter_document();
    let mut runner = TestRunner::deterministic();
    let (mut parsed, mut sequences, mut mappings) = (0, 0, 0);
    let (mut textless_elements, mut textless_keys, mut required) = (0, 0, 0);
    let mut empty_key_elements = 0;
    for _ in 0..SAMPLES {
        let source = strategy
            .new_tree(&mut runner)
            .expect("the strategy generates a document")
            .current();
        let document = parse_markdown(&source, MarkdownOptions::default());
        let DocumentFrontmatter::Mapping { value, anchors, .. } = &document.frontmatter else {
            continue;
        };
        parsed += 1;
        required += assert_written_entries_keep_anchors(&source, value, anchors);
        let holds = |spellings: &[&str]| holds_spelling(&source, spellings);
        if source.contains("\n  -") {
            sequences += 1;
            if holds(&ARBITRARY_ELEMENTS[..ARBITRARY_TEXTLESS_ELEMENTS]) {
                textless_elements += 1;
            }
            if holds(&ARBITRARY_ELEMENTS[ARBITRARY_ELEMENTS.len() - ARBITRARY_EMPTY_KEY_ELEMENTS..])
            {
                empty_key_elements += 1;
            }
        }
        if source.contains("\n  next: 2") {
            mappings += 1;
            if holds(&ARBITRARY_KEYS[..ARBITRARY_TEXTLESS_KEYS]) {
                textless_keys += 1;
            }
        }
    }
    println!(
        "of {SAMPLES} generated documents: {parsed} parsed as a mapping, \
         {sequences} held a block sequence ({textless_elements} of them a textless \
         element, {empty_key_elements} of them a mapping under a quoted empty key), \
         {mappings} held a nested mapping ({textless_keys} of them a \
         textless key); {required} written entries had to keep an anchor"
    );

    assert!(parsed >= SAMPLES / 4, "only {parsed} documents parsed");
    assert!(
        sequences >= SAMPLES / 16,
        "only {sequences} documents held a block sequence"
    );
    assert!(
        textless_elements >= SAMPLES / 32,
        "only {textless_elements} documents held a textless element"
    );
    // A mapping under a quoted empty key is the one element whose position
    // comes from its first key rather than from its own span, and the corpus
    // once lacked it entirely — which let a change to that preference look
    // equivalent over every document this generator could produce.
    assert!(
        empty_key_elements >= SAMPLES / 32,
        "only {empty_key_elements} documents held a mapping under a quoted empty key"
    );
    assert!(
        mappings >= SAMPLES / 16,
        "only {mappings} documents held a nested mapping"
    );
    assert!(
        textless_keys >= SAMPLES / 32,
        "only {textless_keys} documents held a textless key"
    );
    assert!(
        required >= SAMPLES,
        "only {required} written entries were required to keep an anchor"
    );
}
